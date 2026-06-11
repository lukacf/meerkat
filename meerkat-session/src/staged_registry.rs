//! `StagedSessionRegistry` — the single typed owner of session
//! materialization status, staged-capacity custody, and the global
//! active-capacity admission seam.
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
//! `StagedSessionRegistry` folds the facts behind **one** lock so that:
//!
//! 1. [`MaterializationStatus`] is a singular typed fact owned here, never
//!    re-derived from permit-existence + handle-existence elsewhere;
//! 2. the *staged* capacity permit is held **inside** the registry record —
//!    "this session is staged" and "this session's reserved capacity token"
//!    are one fact under one lock, not a status map mirrored beside a
//!    handle-side permit slot; and
//! 3. the staged→active promotion verdict is owned here:
//!    [`Self::begin_promotion`] is the single-winner gate, and an in-flight
//!    promotion is settled (committed or restored) through the same owner.
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
    /// status is held for the duration of the promotion seam (until the run
    /// genuinely begins) and guards against a concurrent second promotion of
    /// the same reservation. An uncommitted promotion settles back to
    /// `Staged`.
    Promoting,
    /// Live work currently holds capacity for this session.
    Active,
}

/// Typed materialization record owned by the registry.
///
/// Unlike the public [`MaterializationStatus`] projection, the record fuses
/// staged-ness with custody of the reserved capacity permit: a `Staged`
/// session's permit lives *here*, behind the same lock as the status.
enum MaterializationRecord {
    /// Staged reservation. `permit` is `None` only for an unbounded registry
    /// (no capacity gate configured).
    Staged {
        permit: Option<OwnedSemaphorePermit>,
    },
    /// Promotion in flight; permit custody has been handed to the winner's
    /// active-capacity lease.
    Promoting,
    /// Live work holds capacity (the permit lives in the session's
    /// active-capacity lease).
    Active,
}

impl MaterializationRecord {
    fn status(&self) -> MaterializationStatus {
        match self {
            Self::Staged { .. } => MaterializationStatus::Staged,
            Self::Promoting => MaterializationStatus::Promoting,
            Self::Active => MaterializationStatus::Active,
        }
    }
}

/// Outcome of a live admission at the registry's write-locked seam
/// ([`StagedSessionRegistry::reserve`]).
///
/// The permit (when present) is handed back to the caller so it can be
/// threaded into the per-session active-capacity lease. A `None` permit means
/// the registry has no global capacity gate configured (unbounded), which is a
/// valid "admitted without a token" outcome — distinct from "rejected".
pub struct AdmissionOutcome {
    /// The status the session now holds in the registry.
    pub status: MaterializationStatus,
    /// The capacity permit consumed for this admission, when a gate is
    /// configured. `None` means the registry is unbounded.
    pub permit: Option<OwnedSemaphorePermit>,
}

/// Single-winner staged→active promotion grant returned by
/// [`StagedSessionRegistry::begin_promotion`].
///
/// Receiving this value means the caller won the `Staged -> Promoting`
/// transition and now holds custody of the staged capacity permit (`None`
/// only for an unbounded registry).
pub struct StagedPromotion {
    /// The staged capacity permit whose custody transfers to the winner's
    /// active-capacity lease.
    pub permit: Option<OwnedSemaphorePermit>,
}

/// Capability handle bound to an active-capacity lease while a staged→active
/// promotion is in flight.
///
/// The ticket carries no decision of its own — settling consults the
/// registry-owned status: an uncommitted promotion (`Promoting`) restores the
/// staged reservation; a committed one (`Active`) lets the permit drop back to
/// the gate.
pub(crate) struct PromotionTicket {
    registry: Arc<StagedSessionRegistry>,
    id: SessionId,
}

impl PromotionTicket {
    pub(crate) fn new(registry: Arc<StagedSessionRegistry>, id: SessionId) -> Self {
        Self { registry, id }
    }

    /// Commit the promotion: the run genuinely began, so the registry-owned
    /// status flips `Promoting -> Active`.
    pub(crate) fn commit(self) {
        self.registry.complete_promotion(&self.id);
    }

    /// Settle the promotion at final lease release with the lease's capacity
    /// permit. See [`StagedSessionRegistry::finish_promotion`].
    pub(crate) fn settle(self, permit: Option<OwnedSemaphorePermit>) {
        self.registry.finish_promotion(&self.id, permit);
    }
}

/// The single owner of materialization status, staged-permit custody, and the
/// global active-capacity admission seam.
///
/// All status transitions and permit reservations flow through this type's
/// write-lock, so the facts can never disagree across a window.
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
    records: IndexMap<SessionId, MaterializationRecord>,
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

    fn try_acquire_permit(&self) -> Result<Option<OwnedSemaphorePermit>, SessionError> {
        match self.capacity.as_ref() {
            Some(capacity) => match Arc::clone(capacity).try_acquire_owned() {
                Ok(permit) => Ok(Some(permit)),
                Err(_) => Err(self.capacity_exhausted_error()),
            },
            None => Ok(None),
        }
    }

    /// Live admission for an existing session starting live work without a
    /// staged reservation: reserve global capacity and record
    /// [`MaterializationStatus::Active`] under one lock. The permit is handed
    /// back so the caller can thread it into the session's active-capacity
    /// lease.
    ///
    /// Fail-closed: when the capacity gate is exhausted this returns a typed
    /// error and records no status — there is no partial/laundered admission.
    pub fn reserve(&self, id: &SessionId) -> Result<AdmissionOutcome, SessionError> {
        let mut inner = self.lock();
        let permit = self.try_acquire_permit()?;
        inner
            .records
            .insert(id.clone(), MaterializationRecord::Active);
        Ok(AdmissionOutcome {
            status: MaterializationStatus::Active,
            permit,
        })
    }

    /// Reserve a global capacity permit without yet recording a session-keyed
    /// status. Used by the session-create path, where the permit must be
    /// reserved before the agent (and therefore the canonical `SessionId`) is
    /// built. The caller deposits the permit back with
    /// [`Self::record_staged`] (deferred first turn) or threads it into the
    /// active-capacity lease and records [`Self::record_active`] (eager first
    /// turn) once the id is known.
    ///
    /// Fail-closed: a full gate returns the typed exhaustion error, never a
    /// silent "admitted without a permit".
    pub fn reserve_capacity(&self) -> Result<Option<OwnedSemaphorePermit>, SessionError> {
        // Hold the registry lock across the permit acquisition so capacity
        // accounting and status records cannot interleave with a concurrent
        // promotion/forget on the same gate.
        let _inner = self.lock();
        self.try_acquire_permit()
    }

    /// Deposit a staged reservation: the registry takes **custody** of the
    /// reserved capacity permit and records
    /// [`MaterializationStatus::Staged`] under one lock. `permit` is `None`
    /// only for an unbounded registry.
    pub fn record_staged(&self, id: &SessionId, permit: Option<OwnedSemaphorePermit>) {
        self.lock()
            .records
            .insert(id.clone(), MaterializationRecord::Staged { permit });
    }

    /// Record [`MaterializationStatus::Active`] for an eagerly-materialized
    /// session whose capacity permit lives in its active-capacity lease.
    pub fn record_active(&self, id: &SessionId) {
        self.lock()
            .records
            .insert(id.clone(), MaterializationRecord::Active);
    }

    /// Single-winner staged→active promotion gate.
    ///
    /// Flips `Staged -> Promoting` inside the registry lock and hands the
    /// staged permit's custody to the winner. Returns `None` when the session
    /// is not `Staged` (already promoting/active, or untracked) — the caller
    /// must fall through to the non-staged admission path.
    pub fn begin_promotion(&self, id: &SessionId) -> Option<StagedPromotion> {
        let mut inner = self.lock();
        let record = inner.records.get_mut(id)?;
        match record {
            MaterializationRecord::Staged { permit } => {
                let permit = permit.take();
                *record = MaterializationRecord::Promoting;
                Some(StagedPromotion { permit })
            }
            _ => None,
        }
    }

    /// Commit an in-flight promotion: `Promoting -> Active`. No-op for any
    /// other status (e.g. the record was concurrently forgotten), so it can
    /// never create a phantom `Active` record.
    pub fn complete_promotion(&self, id: &SessionId) {
        let mut inner = self.lock();
        if let Some(record) = inner.records.get_mut(id)
            && matches!(record, MaterializationRecord::Promoting)
        {
            *record = MaterializationRecord::Active;
        }
    }

    /// Settle a promotion at final lease release.
    ///
    /// If the promotion is still uncommitted (`Promoting` — the run never
    /// genuinely began), the staged reservation is restored: the status flips
    /// back to `Staged` and the registry re-takes custody of the capacity
    /// permit. Otherwise the permit drops here and capacity returns to the
    /// gate. One lock, one decision — the registry-owned status *is* the
    /// restore-vs-release verdict.
    pub fn finish_promotion(&self, id: &SessionId, permit: Option<OwnedSemaphorePermit>) {
        let mut inner = self.lock();
        if let Some(record) = inner.records.get_mut(id)
            && matches!(record, MaterializationRecord::Promoting)
        {
            *record = MaterializationRecord::Staged { permit };
            return;
        }
        // Committed (or concurrently forgotten) promotion: the permit drops
        // here and capacity returns to the gate.
        drop(permit);
    }

    /// Current typed materialization status for a session, if the registry
    /// tracks it.
    pub fn status(&self, id: &SessionId) -> Option<MaterializationStatus> {
        self.lock()
            .records
            .get(id)
            .map(MaterializationRecord::status)
    }

    /// Drop the registry's record for a session. For a staged session this
    /// also drops the registry-held capacity permit, returning capacity to the
    /// gate; an active session's permit is released by its lease on the caller
    /// side.
    pub fn forget(&self, id: &SessionId) {
        self.lock().records.swap_remove(id);
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
    fn reserve_consumes_permit_and_records_active_status() {
        let registry = StagedSessionRegistry::bounded(1);
        let id = sid();

        let outcome = registry.reserve(&id).expect("first reservation admitted");
        assert_eq!(outcome.status, MaterializationStatus::Active);
        assert!(
            outcome.permit.is_some(),
            "bounded registry hands back the consumed permit"
        );
        // The typed status is owned by the registry, not re-derived elsewhere.
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Active));
    }

    #[test]
    fn reserve_fails_closed_when_capacity_exhausted() {
        let registry = StagedSessionRegistry::bounded(1);
        let held = registry
            .reserve(&sid())
            .expect("first reservation admitted");
        // Holding the permit keeps the gate closed.
        assert!(held.permit.is_some());

        let err = registry
            .reserve(&sid())
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
                .reserve(&sid())
                .expect("first reservation admitted");
            drop(outcome);
        }
        // After the permit drops, the next reservation succeeds again.
        registry
            .reserve(&sid())
            .expect("capacity freed after permit drop");
    }

    #[test]
    fn record_staged_takes_permit_custody() {
        let registry = StagedSessionRegistry::bounded(1);
        let id = sid();
        let permit = registry.reserve_capacity().expect("capacity reserved");
        assert!(permit.is_some());
        registry.record_staged(&id, permit);

        assert_eq!(registry.status(&id), Some(MaterializationStatus::Staged));
        // The registry holds the staged permit, so the gate stays closed.
        registry
            .ensure_capacity_available()
            .expect_err("staged custody keeps the gate closed");
        // Forgetting the staged session drops the registry-held permit.
        registry.forget(&id);
        registry
            .ensure_capacity_available()
            .expect("capacity freed when the staged record is forgotten");
    }

    #[test]
    fn begin_promotion_is_single_winner_and_transfers_custody() {
        let registry = StagedSessionRegistry::bounded(2);
        let id = sid();
        let permit = registry.reserve_capacity().expect("capacity reserved");
        registry.record_staged(&id, permit);

        let promotion = registry
            .begin_promotion(&id)
            .expect("first promotion of a staged reservation wins");
        assert!(
            promotion.permit.is_some(),
            "the staged permit's custody transfers to the winner"
        );
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Promoting));
        assert!(
            registry.begin_promotion(&id).is_none(),
            "second concurrent promotion of the same reservation is rejected"
        );
    }

    #[test]
    fn begin_promotion_rejects_non_staged_sessions() {
        let registry = StagedSessionRegistry::bounded(2);
        let id = sid();
        // Untracked session: no promotion.
        assert!(registry.begin_promotion(&id).is_none());
        // Active session: no promotion.
        let _held = registry.reserve(&id).expect("reservation admitted");
        assert!(registry.begin_promotion(&id).is_none());
    }

    #[test]
    fn uncommitted_promotion_settles_back_to_staged() {
        let registry = StagedSessionRegistry::bounded(1);
        let id = sid();
        let permit = registry.reserve_capacity().expect("capacity reserved");
        registry.record_staged(&id, permit);

        let promotion = registry.begin_promotion(&id).expect("promotion wins");
        // Pre-run failure: the run never began, so the final lease release
        // settles the promotion — the staged reservation is restored.
        registry.finish_promotion(&id, promotion.permit);
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Staged));
        registry
            .ensure_capacity_available()
            .expect_err("restored staged reservation re-takes permit custody");
    }

    #[test]
    fn committed_promotion_settles_to_released_capacity() {
        let registry = StagedSessionRegistry::bounded(1);
        let id = sid();
        let permit = registry.reserve_capacity().expect("capacity reserved");
        registry.record_staged(&id, permit);

        let promotion = registry.begin_promotion(&id).expect("promotion wins");
        registry.complete_promotion(&id);
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Active));
        // The run committed; settling releases capacity instead of re-staging.
        registry.finish_promotion(&id, promotion.permit);
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Active));
        registry
            .ensure_capacity_available()
            .expect("committed promotion returns capacity to the gate");
    }

    #[test]
    fn complete_promotion_only_transitions_promoting_sessions() {
        let registry = StagedSessionRegistry::bounded(2);
        let id = sid();
        // Untracked session: no phantom Active record.
        registry.complete_promotion(&id);
        assert_eq!(registry.status(&id), None);

        // Staged (not promoting) session: status is unchanged.
        let permit = registry.reserve_capacity().expect("capacity reserved");
        registry.record_staged(&id, permit);
        registry.complete_promotion(&id);
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Staged));
    }

    #[test]
    fn forget_clears_typed_status() {
        let registry = StagedSessionRegistry::bounded(2);
        let id = sid();
        let _held = registry.reserve(&id).expect("reservation admitted");
        registry.forget(&id);
        assert_eq!(registry.status(&id), None);
    }

    #[test]
    fn ensure_capacity_available_fails_closed() {
        let registry = StagedSessionRegistry::bounded(1);
        registry
            .ensure_capacity_available()
            .expect("spare capacity");
        let _held = registry.reserve(&sid()).expect("reservation admitted");
        registry
            .ensure_capacity_available()
            .expect_err("must fail closed once the gate is full");
    }

    #[test]
    fn unbounded_registry_admits_without_permit() {
        let registry = StagedSessionRegistry::unbounded();
        let outcome = registry
            .reserve(&sid())
            .expect("unbounded registry always admits");
        assert!(
            outcome.permit.is_none(),
            "unbounded registry has no permit to hand back"
        );
        registry
            .ensure_capacity_available()
            .expect("unbounded registry never exhausts");
    }

    #[test]
    fn unbounded_staged_promotion_is_status_gated_not_permit_gated() {
        let registry = StagedSessionRegistry::unbounded();
        let id = sid();
        registry.record_staged(&id, None);
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Staged));

        // Promotion is gated by the registry-owned status, not by permit
        // presence: an unbounded staged session still wins exactly once.
        let promotion = registry
            .begin_promotion(&id)
            .expect("unbounded staged session promotes by status");
        assert!(promotion.permit.is_none());
        assert!(registry.begin_promotion(&id).is_none());
        registry.complete_promotion(&id);
        assert_eq!(registry.status(&id), Some(MaterializationStatus::Active));
    }
}
