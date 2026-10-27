//! One actual native write's retained Console-entry reservation.
//! The native owner calls enter after its waits; no identity is decoded here.

use std::sync::{Arc, Mutex};
use meerkat_core::{AdministrationEntry, AdministrationEntryRefusal, EnteredAdministration};
use toolkit_mobkit::private_host::ConsoleLoginReservation;
use super::super::ReviewCancellation;

pub(crate) type AdministrationCheck = Arc<dyn Fn() -> Result<ConsoleLoginReservation,
    AdministrationEntryRefusal> + Send + Sync>;
pub(crate) type CaptureAdministration<'a> = dyn Fn() -> Result<AdministrationCheck,
    crate::ConnectError> + Send + Sync + 'a;

#[derive(Default)]
struct Reservation {
    entered: bool,
    held: Option<ConsoleLoginReservation>,
}

/// Per-call custody only. The caller and opaque native permit share this exact
/// reservation. Dropping the caller never releases an in-flight native hold.
/// An abandoned or unknown result keeps the existing login uncertainty bit.
pub(super) struct AdministrationAttempt {
    check: AdministrationCheck,
    cancellation: ReviewCancellation,
    reservation: Arc<Mutex<Reservation>>,
}
impl AdministrationAttempt {
    pub(super) fn new(check: AdministrationCheck, cancellation: ReviewCancellation) -> Arc<Self> {
        Arc::new(Self { check, cancellation, reservation: Arc::new(Mutex::new(Reservation::default())) })
    }

    /// Call only after the native future returns its actual settled outcome.
    pub(super) fn settled(&self, unknown: bool) {
        if let Ok(mut state) = self.reservation.lock() {
            if let Some(held) = state.held.take() {
                if unknown { held.uncertain(); } else { held.settled(); }
            }
        }
    }
}
impl AdministrationEntry for AdministrationAttempt {
    fn enter(&self) -> Result<EnteredAdministration, AdministrationEntryRefusal> {
        self.cancellation.check().map_err(|_| unavailable())?;
        let mut state = self.reservation.try_lock().map_err(|_| unavailable())?;
        if state.entered { return Err(unavailable()); }
        // The callback uses only the retained accepted carrier, current native
        // access, nonblocking installed phase and same-login try_reserve.
        let held = (self.check)()?;
        state.entered = true;
        state.held = Some(held);
        Ok(EnteredAdministration::holding(self.reservation.clone()))
    }
}
fn unavailable() -> AdministrationEntryRefusal {
    AdministrationEntryRefusal::Unavailable("The retained credential mutation entry is unavailable".into())
}
