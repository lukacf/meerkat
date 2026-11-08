//! Observe-only contract for one session registration's live MCP adapter.
//!
//! The runtime registration owns an [`McpObservationSlot`]; the agent factory
//! installs the MCP adapter of each construction it successfully finishes for
//! that registration (replacing the previous one, as a same-registration
//! rebuild does), and the registration retires the slot when it ends. An
//! observation names what the current local router exposes for one exact raw
//! operation right now. It grants nothing: it is not a permission, a
//! credential or a promise that a later call will be admitted.

use std::sync::{Arc, Mutex, Weak};

/// The nonsecret identity of a connector-bound server's credential binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConnectorIdentity {
    pub realm_id: String,
    pub slot_id: String,
    pub issuer: String,
    pub client: String,
    pub account: String,
}

/// What a session's current MCP router exposes for one exact
/// `(server, raw_operation)` now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpOperationObservation {
    /// No current route serves the operation, and the server has a staged or
    /// in-progress connection. A staged server is never callable.
    Pending,
    /// No such server or operation is currently exposed, and nothing is
    /// staged or connecting for it.
    Absent,
    /// The server's latest accepted connection attempt failed and nothing
    /// newer is staged or connecting.
    Failed,
    /// A current route serves the operation on this live connection.
    Callable {
        /// The physical generation of the connection serving it.
        connection_generation: u64,
        /// The connector binding of that same connection, if it has one.
        connector: Option<McpConnectorIdentity>,
        /// The service-secret writes that same connection's stdio server was
        /// launched with, if its launch injected any: every call over the
        /// connection is served by these writes' values.
        service_secret_launch: Option<crate::auth::McpServiceSecretLaunch>,
    },
    /// The router has been shut down.
    Retired,
}

/// An MCP adapter that can report what its router currently exposes.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait McpOperationObserver: Send + Sync {
    async fn observe_operation(&self, server: &str, raw_operation: &str)
    -> McpOperationObservation;
}

/// One installation of an observer in a registration's slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpObservationInstallation(u64);

#[derive(Default)]
struct SlotState {
    next: u64,
    /// The installed observer is held weakly: the slot follows the actual
    /// installation through its counter but never keeps a router alive past
    /// the agent that owns it.
    current: Option<(McpObservationInstallation, Weak<dyn McpOperationObserver>)>,
    retired: bool,
}

/// A session registration's MCP observation slot. Every registration owns a
/// fresh one; clones share it.
#[derive(Clone, Default)]
pub struct McpObservationSlot(Arc<Mutex<SlotState>>);

impl std::fmt::Debug for McpObservationSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state();
        f.debug_struct("McpObservationSlot")
            .field("installed", &state.current.is_some())
            .field("retired", &state.retired)
            .finish()
    }
}

impl McpObservationSlot {
    fn state(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Install the adapter of a construction finished for this registration,
    /// replacing any earlier one. `None` once the registration has ended.
    pub fn install(
        &self,
        observer: Arc<dyn McpOperationObserver>,
    ) -> Option<McpObservationInstallation> {
        let mut state = self.state();
        if state.retired {
            return None;
        }
        state.next += 1;
        let installation = McpObservationInstallation(state.next);
        state.current = Some((installation, Arc::downgrade(&observer)));
        Some(installation)
    }

    /// A construction finished for this registration without an MCP
    /// adapter: the earlier installation, if any, ends.
    pub fn clear(&self) {
        self.state().current = None;
    }

    /// The current installation of a live registration, if any, with its
    /// observer while the agent that owns it is still alive (`None` once it
    /// is gone: its router has been released with it).
    #[allow(clippy::type_complexity)]
    pub fn current(
        &self,
    ) -> Option<(
        McpObservationInstallation,
        Option<Arc<dyn McpOperationObserver>>,
    )> {
        let state = self.state();
        if state.retired {
            return None;
        }
        state
            .current
            .as_ref()
            .map(|(installation, observer)| (*installation, observer.upgrade()))
    }

    /// Whether `installation` is still this live registration's current one.
    pub fn is_current(&self, installation: McpObservationInstallation) -> bool {
        let state = self.state();
        !state.retired
            && state
                .current
                .as_ref()
                .is_some_and(|(current, _)| *current == installation)
    }

    /// Whether `other` is this same registration's slot.
    pub fn same_slot(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The registration ended: retire its slot for good.
    pub fn retire(&self) {
        let mut state = self.state();
        state.retired = true;
        state.current = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    struct Fixed(McpOperationObservation);

    #[async_trait::async_trait]
    impl McpOperationObserver for Fixed {
        async fn observe_operation(&self, _: &str, _: &str) -> McpOperationObservation {
            self.0.clone()
        }
    }

    #[test]
    fn the_slot_never_keeps_an_installed_observer_alive() {
        let slot = McpObservationSlot::default();
        let observer: Arc<dyn McpOperationObserver> =
            Arc::new(Fixed(McpOperationObservation::Absent));
        let installed = slot.install(Arc::clone(&observer)).unwrap();
        assert!(slot.current().unwrap().1.is_some());
        drop(observer);
        let (current, observer) = slot.current().unwrap();
        assert_eq!(current, installed);
        assert!(observer.is_none(), "the owner released it");
    }

    #[test]
    fn a_rebuild_replaces_the_installation_and_retirement_ends_it() {
        let slot = McpObservationSlot::default();
        assert!(slot.current().is_none());
        let kept: Vec<Arc<dyn McpOperationObserver>> = vec![
            Arc::new(Fixed(McpOperationObservation::Absent)),
            Arc::new(Fixed(McpOperationObservation::Pending)),
            Arc::new(Fixed(McpOperationObservation::Absent)),
        ];
        let first = slot.install(Arc::clone(&kept[0])).unwrap();
        assert!(slot.is_current(first));
        let second = slot.install(Arc::clone(&kept[1])).unwrap();
        assert!(
            !slot.is_current(first),
            "a rebuild replaces the installation"
        );
        assert!(slot.is_current(second));
        slot.clear();
        assert!(!slot.is_current(second), "a rebuild without MCP ends it");
        assert!(slot.current().is_none());
        let third = slot.install(Arc::clone(&kept[2])).unwrap();
        assert_ne!(third, second);
        slot.retire();
        assert!(!slot.is_current(third));
        assert!(slot.current().is_none());
        assert!(
            slot.install(Arc::new(Fixed(McpOperationObservation::Absent)))
                .is_none(),
            "nothing installs into an ended registration"
        );
        assert!(slot.same_slot(&slot.clone()));
        assert!(!slot.same_slot(&McpObservationSlot::default()));
    }
}
