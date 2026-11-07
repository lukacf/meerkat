//! One actual native write's retained Console-entry reservation.
//! The native owner calls enter after its waits; no identity is decoded here.

use super::super::ReviewCancellation;
use meerkat_core::approval::consent::HostAuthenticationScope;
use meerkat_core::{
    AdministeringPrincipal, AdministrationEntry, AdministrationEntryRefusal,
    AdministrationRecorder, EnteredAdministration, PrincipalRef,
};
use std::sync::{Arc, Mutex};
use toolkit_mobkit::private_host::ConsoleLoginReservation;

type AdministrationEntryCheck =
    Arc<dyn Fn() -> Result<ConsoleLoginReservation, AdministrationEntryRefusal> + Send + Sync>;
pub(crate) type CaptureAdministration<'a> =
    dyn Fn() -> Result<AdministrationCheck, crate::ConnectError> + Send + Sync + 'a;

/// The Console's admission check for one credential write: the same-login
/// entry it runs at the write's entry, and the administrator it states for
/// the host's recorder, when the host passed one.
#[derive(Clone)]
pub(crate) struct AdministrationCheck {
    entry: AdministrationEntryCheck,
    administrator: Option<StatedAdministrator>,
}
impl AdministrationCheck {
    pub(crate) fn new(
        entry: impl Fn() -> Result<ConsoleLoginReservation, AdministrationEntryRefusal>
        + Send
        + Sync
        + 'static,
        administrator: Option<StatedAdministrator>,
    ) -> Self {
        Self {
            entry: Arc::new(entry),
            administrator,
        }
    }
}

/// The person a Console admission states, as the Console authenticated them
/// (the qualified principal and the login they answered in, kept as its
/// digest only), and the host's recorder that receives the write's record.
#[derive(Clone)]
pub(crate) struct StatedAdministrator {
    administrator: AdministeringPrincipal,
    recorder: Arc<dyn AdministrationRecorder>,
}
impl StatedAdministrator {
    pub(crate) fn new(
        principal: PrincipalRef,
        login_scope: &str,
        recorder: Arc<dyn AdministrationRecorder>,
    ) -> Result<Self, crate::ConnectError> {
        let administrator = AdministeringPrincipal::new(principal)
            .map_err(|_| {
                crate::ConnectError::InvalidInput(
                    "The Console administrator names no trust domain".into(),
                )
            })?
            .with_authentication_scope(&HostAuthenticationScope::new(login_scope));
        Ok(Self {
            administrator,
            recorder,
        })
    }
}

#[derive(Default)]
struct Reservation {
    entered: bool,
    held: Option<ConsoleLoginReservation>,
}

/// Per-call custody only. The caller and opaque native permit share this exact
/// reservation. Dropping the caller never releases an in-flight native hold.
/// An abandoned or unknown result keeps the existing login uncertainty bit.
pub(crate) struct AdministrationAttempt {
    check: AdministrationCheck,
    cancellation: ReviewCancellation,
    reservation: Arc<Mutex<Reservation>>,
}
impl AdministrationAttempt {
    pub(crate) fn new(check: AdministrationCheck, cancellation: ReviewCancellation) -> Arc<Self> {
        Arc::new(Self {
            check,
            cancellation,
            reservation: Arc::new(Mutex::new(Reservation::default())),
        })
    }

    /// Call only after the native future returns its actual settled outcome.
    pub(crate) fn settled(&self, unknown: bool) {
        if let Ok(mut state) = self.reservation.lock()
            && let Some(held) = state.held.take()
        {
            if unknown {
                held.uncertain();
            } else {
                held.settled();
            }
        }
    }
}
impl AdministrationEntry for AdministrationAttempt {
    fn enter(&self) -> Result<EnteredAdministration, AdministrationEntryRefusal> {
        self.cancellation.check().map_err(|_| unavailable())?;
        let mut state = self.reservation.try_lock().map_err(|_| unavailable())?;
        if state.entered {
            return Err(unavailable());
        }
        // The callback uses only the retained accepted carrier, current native
        // access, nonblocking installed phase and same-login try_reserve.
        let held = (self.check.entry)()?;
        state.entered = true;
        state.held = Some(held);
        Ok(admission(
            self.reservation.clone(),
            self.check.administrator.as_ref(),
        ))
    }
}
/// The admission of a write this check entered. It holds the login's
/// reservation until the write settles, and states the person who answered
/// for the host's recorder when the check has one: the write's owner then
/// hands the recorder the write's record when it settles.
fn admission(
    held: Arc<Mutex<Reservation>>,
    administrator: Option<&StatedAdministrator>,
) -> EnteredAdministration {
    let entered = EnteredAdministration::holding(held);
    match administrator {
        Some(stated) => {
            entered.administered_by(stated.administrator.clone(), stated.recorder.clone())
        }
        None => entered,
    }
}
fn unavailable() -> AdministrationEntryRefusal {
    AdministrationEntryRefusal::Unavailable(
        "The retained credential mutation entry is unavailable".into(),
    )
}
