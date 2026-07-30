//! Shared fail-closed health authority for one persistent runtime session.
//!
//! A persistence outcome that cannot be reconciled in place degrades the
//! shared handle to [`DurabilityHealthState::ReloadRequired`]. The handle has
//! deliberately no ordinary path back to Ready: the only capability that can
//! publish readiness is the non-cloneable
//! [`DurabilityRehydrationAuthority`] minted for one registration-authorized
//! cold install.

use std::sync::{Arc, Mutex};

/// Stable reason a persistent runtime must be cold-loaded again before it may
/// execute or mutate durable session state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("durability reload required after `{operation}`: {reason}")]
pub(crate) struct DurabilityReloadRequired {
    operation: String,
    reason: String,
}

impl DurabilityReloadRequired {
    fn new(operation: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn operation(&self) -> &str {
        &self.operation
    }

    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }
}

/// Public projection of the shared durability gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DurabilityHealthState {
    /// Durable state and the live runtime shell are one verified image.
    Ready,
    /// The live shell may no longer be used; registration must cold-load a new
    /// image before execution can resume.
    ReloadRequired(DurabilityReloadRequired),
}

#[derive(Debug)]
struct DurabilityHealthRecord {
    state: DurabilityHealthState,
    /// True only between minting and consuming the one registration cold
    /// install capability. Any real degradation revokes it before replacing
    /// the provisional ReloadRequired reason.
    cold_install_open: bool,
}

/// Cloneable durability gate shared by the runtime entry and its persistent
/// driver.
///
/// The transition surface is intentionally asymmetric. Any owner may degrade
/// the shared session, while no handle clone can restore readiness.
#[derive(Debug, Clone)]
pub(crate) struct DurabilityHealthHandle {
    inner: Arc<Mutex<DurabilityHealthRecord>>,
}

impl DurabilityHealthHandle {
    /// Refuse execution or mutation unless registration published a verified
    /// cold-installed image and no later durability failure degraded it.
    pub(crate) fn require_ready(&self) -> Result<(), DurabilityReloadRequired> {
        match &self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
        {
            DurabilityHealthState::Ready => Ok(()),
            DurabilityHealthState::ReloadRequired(required) => Err(required.clone()),
        }
    }

    /// Degrade this session exactly once. The first real failure is retained
    /// as the reload cause; later callers cannot overwrite its evidence.
    ///
    /// Returns `true` only for the caller that performed the Ready (or
    /// provisional cold-install) -> ReloadRequired transition.
    pub(crate) fn mark_reload_required(
        &self,
        operation: &'static str,
        reason: impl Into<String>,
    ) -> bool {
        let mut record = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !record.cold_install_open
            && matches!(&record.state, DurabilityHealthState::ReloadRequired(_))
        {
            return false;
        }
        record.state =
            DurabilityHealthState::ReloadRequired(DurabilityReloadRequired::new(operation, reason));
        record.cold_install_open = false;
        true
    }

    #[cfg(test)]
    fn state(&self) -> DurabilityHealthState {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
            .clone()
    }
}

/// Sole capability for publishing a persistent runtime as Ready.
///
/// This authority is private to the MeerkatMachine registration implementation
/// and is not Clone. Consuming it succeeds only while its exact cold install is
/// still open; a concurrent degradation revokes it permanently.
pub(super) struct DurabilityRehydrationAuthority {
    handle: DurabilityHealthHandle,
}

impl DurabilityRehydrationAuthority {
    pub(super) fn mark_ready(self) -> Result<(), DurabilityReloadRequired> {
        let mut record = self
            .handle
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !record.cold_install_open {
            return match &record.state {
                DurabilityHealthState::Ready => Ok(()),
                DurabilityHealthState::ReloadRequired(required) => Err(required.clone()),
            };
        }
        record.state = DurabilityHealthState::Ready;
        record.cold_install_open = false;
        Ok(())
    }
}

/// Mint the health gate for one registration-authorized persistent cold
/// install. The returned handle remains fail-closed until the paired authority
/// is consumed after the entire install succeeds.
pub(super) fn begin_registration_cold_install()
-> (DurabilityHealthHandle, DurabilityRehydrationAuthority) {
    let handle = DurabilityHealthHandle {
        inner: Arc::new(Mutex::new(DurabilityHealthRecord {
            state: DurabilityHealthState::ReloadRequired(DurabilityReloadRequired::new(
                "registration_cold_install",
                "persistent runtime cold install has not published readiness",
            )),
            cold_install_open: true,
        })),
    };
    let authority = DurabilityRehydrationAuthority {
        handle: handle.clone(),
    };
    (handle, authority)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_install_is_the_only_ready_transition_and_degradation_is_monotonic() {
        let (health, rehydration) = begin_registration_cold_install();
        assert!(health.require_ready().is_err());

        rehydration.mark_ready().unwrap();
        assert_eq!(health.state(), DurabilityHealthState::Ready);
        assert!(health.require_ready().is_ok());

        assert!(health.mark_reload_required("atomic_apply", "outcome unknown"));
        assert!(!health.mark_reload_required("later_retry", "must not replace first cause"));
        let required = health.require_ready().unwrap_err();
        assert_eq!(required.operation(), "atomic_apply");
        assert_eq!(required.reason(), "outcome unknown");
    }

    #[test]
    fn degradation_during_cold_install_revokes_rehydration_authority() {
        let (health, rehydration) = begin_registration_cold_install();
        assert!(health.mark_reload_required("recovery_cas", "durable image changed"));

        let error = rehydration.mark_ready().unwrap_err();
        assert_eq!(error.operation(), "recovery_cas");
        assert_eq!(health.require_ready().unwrap_err(), error);
    }
}
