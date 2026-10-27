//! Mechanical lifetime forwarded by the trusted parent on this exact pipe.
//! Enrollment labels do not authenticate a human or grant any authority. A
//! native verified carrier must select the exact tuple before retaining a handle.

use super::PrivateHostClosed;
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_mobkit::identity_first::{
    GovernedEntryFence, GovernedEntryOutcome, GovernedEntryPermit, GovernedEntryRefusal,
};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

const MAX_ENROLLMENTS: usize = 256;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConsoleLoginSelection {
    pub issuer: String,
    pub subject: String,
    pub login_scope_id: String,
    pub expires_at_ms: u64,
}

impl ConsoleLoginSelection {
    fn validate(&self) -> Result<(), HostLifetimeError> {
        for (value, bound) in [
            (&self.issuer, 4096),
            (&self.subject, 4096),
            (&self.login_scope_id, 256),
        ] {
            if value.trim().is_empty() || value.len() > bound || value.chars().any(char::is_control)
            {
                return Err(HostLifetimeError::InvalidSelection);
            }
        }
        if self.expires_at_ms == 0 || self.expires_at_ms > MAX_SAFE_INTEGER {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(())
    }

    fn same_login(&self, issuer: &str, subject: &str, scope: &str) -> bool {
        self.issuer == issuer && self.subject == subject && self.login_scope_id == scope
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleLoginRefusal {
    Retired,
    Expired,
    Unavailable,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConsoleLoginRetirement {
    pub fenced: bool,
    pub reservations_drained: bool,
    pub uncertain: bool,
}

#[derive(Default)]
struct State {
    retired: bool,
    reservations: usize,
    uncertain: bool,
    waiting: bool,
}

struct Login {
    selection: ConsoleLoginSelection,
    closed: PrivateHostClosed,
    state: Mutex<State>,
    changed: watch::Sender<()>,
}

/// Selected once from native verified facts, then retained across waits.
/// Do not resolve a replacement handle after a protected operation has entered.
#[derive(Clone)]
pub struct ConsoleLoginHandle(Arc<Login>);

/// This source belongs to one private connection. It contains only its live
/// transport enrollments, not login policy, principals, or operation outcomes.
#[derive(Clone)]
pub struct ConsoleLogins {
    closed: PrivateHostClosed,
    publication: Option<LocalAuthorizationPublication>,
    rows: Arc<Mutex<BTreeMap<u64, ConsoleLoginHandle>>>,
}

fn now_ms() -> Result<u64, ConsoleLoginRefusal> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| u64::try_from(value.as_millis()).ok())
        .ok_or(ConsoleLoginRefusal::Unavailable)
}

impl ConsoleLogins {
    /// Mechanical owner fixture only. It supplies no native Human or grant.
    #[cfg(test)]
    pub(crate) fn fixture_owner() -> (watch::Sender<bool>, Self) {
        let (send, receive) = watch::channel(false);
        (send, Self::new(PrivateHostClosed(receive), None))
    }

    #[cfg(test)]
    pub(crate) fn fixture_enroll(&self, id: u64, selection: ConsoleLoginSelection)
        -> Result<ConsoleLoginHandle, HostLifetimeError>
    {
        self.enroll(id, selection.clone())?;
        self.resolve(&selection.issuer, &selection.subject, &selection.login_scope_id)
            .map_err(|_| HostLifetimeError::Unavailable)
    }

    #[cfg(test)]
    pub(crate) fn fixture_retire(&self, id: u64)
        -> Result<std::pin::Pin<Box<dyn std::future::Future<Output = Result<ConsoleLoginRetirement, HostLifetimeError>> + Send>>, HostLifetimeError>
    {
        Ok(Box::pin(self.retire(id)?.drain()))
    }
    pub(super) fn new(
        closed: PrivateHostClosed,
        publication: Option<LocalAuthorizationPublication>,
    ) -> Self {
        Self {
            closed,
            publication,
            rows: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(super) fn enroll(
        &self,
        id: u64,
        selection: ConsoleLoginSelection,
    ) -> Result<(), HostLifetimeError> {
        selection.validate()?;
        if self.closed.is_closed() {
            return Err(HostLifetimeError::Unavailable);
        }
        let now = now_ms().map_err(|_| HostLifetimeError::Unavailable)?;
        if selection.expires_at_ms <= now {
            return Err(HostLifetimeError::Denied);
        }
        // Publish before changing enrollment facts. This source has no phase
        // mutex; activation/component owners retain their phase-first order.
        let _publication = self.publication.as_ref()
            .map(LocalAuthorizationPublication::begin_owner_change)
            .transpose()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if self.closed.is_closed() {
            return Err(HostLifetimeError::Unavailable);
        }
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        // Expired, drained labels cannot authenticate another token or retain an
        // active reservation. Keep retired but unexpired labels to prevent reuse.
        rows.retain(|_, handle| {
            handle.0.selection.expires_at_ms > now
                || handle
                    .0
                    .state
                    .lock()
                    .map_or(true, |state| state.reservations != 0)
        });
        if rows.contains_key(&id)
            || rows.values().any(|handle| {
                handle.0.selection.same_login(
                    &selection.issuer,
                    &selection.subject,
                    &selection.login_scope_id,
                )
            })
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        if rows.len() >= MAX_ENROLLMENTS {
            return Err(HostLifetimeError::Unavailable);
        }
        let (changed, _) = watch::channel(());
        rows.insert(
            id,
            ConsoleLoginHandle(Arc::new(Login {
                selection,
                closed: self.closed.clone(),
                state: Mutex::new(State::default()),
                changed,
            })),
        );
        Ok(())
    }

    /// The caller supplies the issuer, subject and signed scope from the actual
    /// native verified carrier, never an email-first display principal.
    pub fn resolve(
        &self,
        issuer: &str,
        subject: &str,
        scope: &str,
    ) -> Result<ConsoleLoginHandle, ConsoleLoginRefusal> {
        if self.closed.is_closed() {
            return Err(ConsoleLoginRefusal::Unavailable);
        }
        let rows = self
            .rows
            .lock()
            .map_err(|_| ConsoleLoginRefusal::Unavailable)?;
        let handle = rows
            .values()
            .find(|handle| handle.0.selection.same_login(issuer, subject, scope))
            .ok_or(ConsoleLoginRefusal::Unknown)?
            .clone();
        handle.check()?;
        Ok(handle)
    }

    /// Protected-parent lifetime lookup. The parent separately owns actual
    /// authentication/administration; this never resolves a verified carrier.
    pub(super) fn parent_enrollment(&self, id: u64) -> Result<ConsoleLoginHandle, HostLifetimeError> {
        if self.closed.is_closed() { return Err(HostLifetimeError::Unavailable); }
        let handle = self.rows.lock().map_err(|_| HostLifetimeError::Unavailable)?
            .get(&id).cloned().ok_or(HostLifetimeError::WrongOwner)?;
        handle.check().map_err(|error| match error {
            ConsoleLoginRefusal::Unavailable => HostLifetimeError::Unavailable,
            _ => HostLifetimeError::Denied,
        })?;
        Ok(handle)
    }

    pub(super) fn retire(&self, id: u64) -> Result<RetirementWait, HostLifetimeError> {
        let _publication = self.publication.as_ref()
            .map(LocalAuthorizationPublication::begin_owner_change)
            .transpose()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let rows = self
            .rows
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let handle = rows.get(&id).ok_or(HostLifetimeError::WrongOwner)?.clone();
        let mut state = handle
            .0
            .state
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        // This synchronous fence must precede all queue/capacity checks and all
        // awaits. The reader never waits for the presenter to acknowledge it.
        state.retired = true;
        handle.0.changed.send_replace(());
        if state.waiting {
            return Err(HostLifetimeError::NotSettled);
        }
        state.waiting = true;
        drop(state);
        Ok(RetirementWait(handle))
    }
}

impl ConsoleLoginHandle {
    /// Exact process owner comparison, never equality of the forwarded labels.
    pub fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    fn check_locked(&self, state: &State) -> Result<(), ConsoleLoginRefusal> {
        if self.0.closed.is_closed() {
            return Err(ConsoleLoginRefusal::Unavailable);
        }
        if state.retired {
            return Err(ConsoleLoginRefusal::Retired);
        }
        if now_ms()? >= self.0.selection.expires_at_ms {
            return Err(ConsoleLoginRefusal::Expired);
        }
        Ok(())
    }

    pub fn check(&self) -> Result<(), ConsoleLoginRefusal> {
        let state = self
            .0
            .state
            .lock()
            .map_err(|_| ConsoleLoginRefusal::Unavailable)?;
        self.check_locked(&state)
    }

    /// Check this same owner without waiting for the login-state mutex.
    /// The existing pipe observer keeps its short watch synchronization.
    /// Contention and poisoned state remain unavailable, not retired/expired.
    pub(crate) fn try_check(&self) -> Result<(), ConsoleLoginRefusal> {
        let state = self.0.state.try_lock().map_err(|_| ConsoleLoginRefusal::Unavailable)?;
        self.check_locked(&state)
    }

    /// Execute one synchronous native revocation while this actual login and
    /// pipe remain open. Caller already holds the installed host phase fence.
    /// Retirement takes publication before state, and pipe close takes it before
    /// the watch write. The callback may only use native nonblocking mutation:
    /// its try-reservation refuses on that contention, releases these guards,
    /// and lets retirement finish. Never await, reserve/check this login again,
    /// resolve a replacement login, or take a blocking publication writer here.
    pub(crate) fn with_current_revoke<T>(&self, operation: impl FnOnce() -> T)
        -> Result<T, ConsoleLoginRefusal>
    {
        let closed = self.0.closed.0.borrow();
        if *closed || self.0.closed.0.has_changed().is_err() {
            return Err(ConsoleLoginRefusal::Unavailable);
        }
        let state = self.0.state.try_lock().map_err(|_| ConsoleLoginRefusal::Unavailable)?;
        if state.retired { return Err(ConsoleLoginRefusal::Retired); }
        if now_ms()? >= self.0.selection.expires_at_ms { return Err(ConsoleLoginRefusal::Expired); }
        Ok(operation())
    }

    /// Reserve before first polling a protected entry. This is not native
    /// admission. Keep the same reservation through its actual settlement.
    pub fn reserve(&self) -> Result<ConsoleLoginReservation, ConsoleLoginRefusal> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| ConsoleLoginRefusal::Unavailable)?;
        self.check_locked(&state)?;
        state.reservations = state
            .reservations
            .checked_add(1)
            .ok_or(ConsoleLoginRefusal::Unavailable)?;
        Ok(ConsoleLoginReservation {
            handle: self.clone(),
            finished: false,
        })
    }

    /// Reserve this same login at a native mutation entry without waiting for
    /// its state lock. This creates no new enrollment or native permission.
    /// Retain the reservation until the actual mutation outcome is settled.
    pub fn try_reserve(&self) -> Result<ConsoleLoginReservation, ConsoleLoginRefusal> {
        let mut state = self.0.state.try_lock().map_err(|_| ConsoleLoginRefusal::Unavailable)?;
        self.check_locked(&state)?;
        state.reservations = state.reservations.checked_add(1)
            .ok_or(ConsoleLoginRefusal::Unavailable)?;
        Ok(ConsoleLoginReservation { handle: self.clone(), finished: false })
    }

    /// Wakes an idle presenter on actual retirement, absolute expiry or terminal
    /// private-pipe loss, even when no new request or HTTP poll arrives.
    pub async fn closed(&self) -> ConsoleLoginRefusal {
        let mut changed = self.0.changed.subscribe();
        loop {
            if let Err(reason) = self.check() {
                return reason;
            }
            let remaining = match now_ms() {
                Ok(now) => self.0.selection.expires_at_ms.saturating_sub(now),
                Err(reason) => return reason,
            };
            tokio::select! {
                _ = changed.changed() => {},
                _ = self.0.closed.wait_closed() => {},
                // Recheck wall-clock expiry after a bounded monotonic wait.
                _ = tokio::time::sleep(Duration::from_millis(remaining.min(60_000))) => {},
            }
        }
    }
}

/// Local count only. Native input, replay, and unresolved settlement remain
/// owned by native custody; no input record is reconstructed here.
pub struct ConsoleLoginReservation {
    handle: ConsoleLoginHandle,
    finished: bool,
}

impl ConsoleLoginReservation {
    pub fn settled(mut self) {
        self.finish(false);
    }
    pub fn uncertain(mut self) {
        self.finish(true);
    }

    fn finish(&mut self, uncertain: bool) {
        if !self.finished {
            if let Ok(mut state) = self.handle.0.state.lock() {
                state.reservations -= 1;
                state.uncertain |= uncertain;
                self.handle.0.changed.send_replace(());
            }
            self.finished = true;
        }
    }
}

impl Drop for ConsoleLoginReservation {
    fn drop(&mut self) {
        self.finish(true);
    }
}

// Native enters this exact retained owner after its member lookup and before
// admission. No new source, callback, identity lease or authorization is made.
impl GovernedEntryFence for ConsoleLoginHandle {
    fn enter(&self) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal> {
        self.reserve()
            .map(|reservation| Box::new(reservation) as Box<dyn GovernedEntryPermit>)
            .map_err(|refusal| match refusal {
                ConsoleLoginRefusal::Retired => GovernedEntryRefusal::Retired,
                ConsoleLoginRefusal::Expired => GovernedEntryRefusal::Expired,
                ConsoleLoginRefusal::Unavailable | ConsoleLoginRefusal::Unknown =>
                    GovernedEntryRefusal::Unavailable("The retained Console login is unavailable".into()),
            })
    }
}

impl GovernedEntryPermit for ConsoleLoginReservation {
    fn finish(mut self: Box<Self>, outcome: GovernedEntryOutcome) {
        // Entered means the admission decision is known, not that accepted
        // work or source publication completed. Those owners remain native.
        ConsoleLoginReservation::finish(self.as_mut(),
            matches!(outcome, GovernedEntryOutcome::Uncertain));
    }
}

pub(super) struct RetirementWait(ConsoleLoginHandle);

impl RetirementWait {
    pub(super) async fn drain(self) -> Result<ConsoleLoginRetirement, HostLifetimeError> {
        let mut changed = self.0.0.changed.subscribe();
        loop {
            if self.0.0.closed.is_closed() {
                return Err(HostLifetimeError::Unavailable);
            }
            {
                let state = self
                    .0
                    .0
                    .state
                    .lock()
                    .map_err(|_| HostLifetimeError::Unavailable)?;
                if state.reservations == 0 {
                    return Ok(ConsoleLoginRetirement {
                        fenced: state.retired,
                        reservations_drained: true,
                        uncertain: state.uncertain,
                    });
                }
            }
            tokio::select! {
                _ = changed.changed() => {},
                _ = self.0.0.closed.wait_closed() => {},
            }
        }
    }
}

impl Drop for RetirementWait {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.0.state.lock() {
            state.waiting = false;
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod parent_lifetime_tests {
    use super::*;
    #[tokio::test]
    async fn parent_id_lookup_is_same_lifetime_and_never_a_verified_carrier() {
        let (send, receive) = watch::channel(false);
        let logins = ConsoleLogins::new(PrivateHostClosed(receive), None);
        let selection = ConsoleLoginSelection { issuer: "https://parent.example".into(),
            subject: "actual-parent-auth-subject".into(), login_scope_id: "one-login".into(),
            expires_at_ms: now_ms().unwrap() + 60_000 };
        logins.enroll(7, selection.clone()).unwrap();
        let held = logins.parent_enrollment(7).unwrap();
        let verified = logins.resolve(&selection.issuer, &selection.subject, &selection.login_scope_id).unwrap();
        assert!(held.same_owner(&verified));
        assert!(matches!(logins.parent_enrollment(8), Err(HostLifetimeError::WrongOwner)));
        assert!(matches!(logins.resolve("wrong", &selection.subject, &selection.login_scope_id), Err(ConsoleLoginRefusal::Unknown)));
        let reservation = held.reserve().unwrap();
        let retired = logins.retire(7).unwrap();
        assert!(matches!(logins.parent_enrollment(7), Err(HostLifetimeError::Denied)));
        reservation.uncertain();
        let observed = retired.drain().await.unwrap();
        assert!(observed.fenced && observed.reservations_drained && observed.uncertain);
        send.send_replace(true);
        assert!(matches!(logins.parent_enrollment(7), Err(HostLifetimeError::Unavailable)));
    }
}
