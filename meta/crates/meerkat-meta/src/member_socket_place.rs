//! One member per gateway under the member socket option.
//!
//! Socket comms (`comms.mode` tcp or uds, which the stdio gateway sets with
//! `runtime_options.member_comms_address`) give the members a signed socket
//! endpoint, and their transport key belongs to the realm rather than to a
//! member's session. A mob admits a transport key for one member, so a
//! gateway with the option serves one member. [`MemberSocketPlace`] is that
//! member's place: the session services of the root mob and of the child
//! mobs share one, and every member build and every durable fork asks it
//! before anything is built or copied.
//!
//! The place is held by a (mob, member) while a create or a fork of that
//! member runs, and afterwards for as long as the session it returned
//! exists. A failed or dropped create gives it back at once; an archived
//! session (a retired member, or a build its mob rolled back) gives it back
//! at the next claim, which reads the session's lifecycle from the session
//! service. Nothing here is durable and nothing fails a start: a refusal is
//! the refused member's own session error. That error names the refused
//! member only: who holds the place is written to the log, once per refused
//! member, never to a caller.

use std::sync::{Arc, Mutex, PoisonError};

use meerkat_core::MobMemberBinding;
use meerkat_core::service::SessionError;
use meerkat_core::types::SessionId;
use meerkat_mob::{MobSessionService, SessionResumeLifecycle};

/// How many refused members one holder is logged for. A refused member may
/// be asked for again and again (a declared member is reconciled with
/// backoff for as long as the holder lives), so each is logged once, and
/// past this many nothing more is logged until the holder changes.
const LOGGED_REFUSALS_PER_HOLDER: usize = 16;

/// The member that holds the place.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Holder {
    mob_id: String,
    member: String,
    /// The holder's session, once a create or a fork returned it.
    session: Option<SessionId>,
    /// Creates and forks of the holder that have not returned yet.
    in_flight: usize,
}

impl Holder {
    fn is(&self, mob_id: &str, member: &str) -> bool {
        self.mob_id == mob_id && self.member == member
    }
}

#[derive(Debug, Default)]
struct Place {
    holder: Option<Holder>,
    /// The (mob, member) pairs whose refusal for the current holder is
    /// already in the log.
    logged_refusals: Vec<(String, String)>,
}

impl Place {
    fn hold(&mut self, holder: Holder) {
        self.holder = Some(holder);
        self.logged_refusals.clear();
    }

    fn free(&mut self) {
        self.holder = None;
        self.logged_refusals.clear();
    }
}

/// What the place answers a claim before any session read.
enum Admission {
    Admitted,
    /// Another member's create or fork is running.
    HeldInFlight(Holder),
    /// Another member holds the place with this session.
    HeldBySession(Holder, SessionId),
}

/// The one member place of a gateway whose members run on socket comms.
#[derive(Debug, Default)]
pub(crate) struct MemberSocketPlace {
    place: Mutex<Place>,
}

impl MemberSocketPlace {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn admit(&self, binding: &MobMemberBinding) -> Admission {
        let mut place = self.place.lock().unwrap_or_else(PoisonError::into_inner);
        let fresh = || Holder {
            mob_id: binding.mob_id.clone(),
            member: binding.member.clone(),
            session: None,
            in_flight: 1,
        };
        match place.holder.as_mut() {
            None => {
                place.hold(fresh());
                Admission::Admitted
            }
            Some(current) if current.is(&binding.mob_id, &binding.member) => {
                current.in_flight += 1;
                Admission::Admitted
            }
            Some(current) if current.in_flight > 0 => Admission::HeldInFlight(current.clone()),
            Some(current) => match current.session.clone() {
                Some(session) => Admission::HeldBySession(current.clone(), session),
                // A holder with neither a session nor a running create is
                // removed when its last create ends, so none is kept here.
                None => {
                    place.hold(fresh());
                    Admission::Admitted
                }
            },
        }
    }

    /// Free the place if `seen` still holds it unchanged.
    fn vacate_if(&self, seen: &Holder) {
        let mut place = self.place.lock().unwrap_or_else(PoisonError::into_inner);
        if place.holder.as_ref() == Some(seen) {
            place.free();
        }
    }

    fn finish(&self, mob_id: &str, member: &str, session: Option<&SessionId>) {
        let mut place = self.place.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(current) = place
            .holder
            .as_mut()
            .filter(|current| current.is(mob_id, member))
        else {
            return;
        };
        current.in_flight = current.in_flight.saturating_sub(1);
        if let Some(session) = session {
            current.session = Some(session.clone());
        }
        if current.in_flight == 0 && current.session.is_none() {
            place.free();
        }
    }

    /// Claim the place for the member `binding` names, before its session is
    /// built or its transcript is forked. `service` is the session service
    /// below the claiming layer: it answers whether another holder's session
    /// still exists.
    ///
    /// # Errors
    /// The refusal ([`refusal_text`]) while another member holds the place.
    pub(crate) async fn claim(
        self: &Arc<Self>,
        service: &dyn MobSessionService,
        binding: &MobMemberBinding,
    ) -> Result<MemberSocketClaim, SessionError> {
        // A holder whose session is gone is replaced; a second look covers
        // a place that changed hands meanwhile, and a third holder is
        // answered as it stands.
        for _ in 0..2 {
            match self.admit(binding) {
                Admission::Admitted => return Ok(self.claimed(binding)),
                Admission::HeldInFlight(holder) => return Err(self.refuse(binding, &holder)),
                Admission::HeldBySession(holder, session) => {
                    if session_exists(service, &session).await {
                        return Err(self.refuse(binding, &holder));
                    }
                    self.vacate_if(&holder);
                }
            }
        }
        match self.admit(binding) {
            Admission::Admitted => Ok(self.claimed(binding)),
            Admission::HeldInFlight(holder) | Admission::HeldBySession(holder, _) => {
                Err(self.refuse(binding, &holder))
            }
        }
    }

    fn claimed(self: &Arc<Self>, binding: &MobMemberBinding) -> MemberSocketClaim {
        MemberSocketClaim {
            place: Arc::clone(self),
            mob_id: binding.mob_id.clone(),
            member: binding.member.clone(),
            ended: false,
        }
    }

    /// The session error `binding`'s member is refused with while `holder`
    /// has the place. The error names the refused member only; the holder
    /// goes to the log, once for each refused member while it holds.
    fn refuse(&self, binding: &MobMemberBinding, holder: &Holder) -> SessionError {
        let log = {
            let mut place = self.place.lock().unwrap_or_else(PoisonError::into_inner);
            let refused = (binding.mob_id.clone(), binding.member.clone());
            let holds = place
                .holder
                .as_ref()
                .is_some_and(|current| current.is(&holder.mob_id, &holder.member));
            if holds
                && place.logged_refusals.len() < LOGGED_REFUSALS_PER_HOLDER
                && !place.logged_refusals.contains(&refused)
            {
                place.logged_refusals.push(refused);
                true
            } else {
                false
            }
        };
        if log {
            tracing::warn!(
                mob_id = %binding.mob_id,
                member = %crate::member_comms_id::runtime_alias_str(&binding.member),
                holder_mob_id = %holder.mob_id,
                holder = %crate::member_comms_id::runtime_alias_str(&holder.member),
                holder_session = ?holder.session,
                "a member was refused before its build: the member socket option serves one \
                 member per gateway and another member holds the place; this member's further \
                 refusals are not logged while that member holds it"
            );
        }
        SessionError::FailedWithData {
            message: refusal_text(&binding.member),
            data: serde_json::Value::Null,
        }
    }
}

/// One running create or fork of the member that holds the place. Dropped
/// without [`Self::built`] (the create failed or was cancelled), it gives
/// the place back unless the member already has a session.
#[must_use = "a claim dropped at once gives the place back"]
pub(crate) struct MemberSocketClaim {
    place: Arc<MemberSocketPlace>,
    mob_id: String,
    member: String,
    ended: bool,
}

impl MemberSocketClaim {
    /// The create or the fork returned the member's session: the member
    /// holds the place while that session exists.
    pub(crate) fn built(mut self, session: &SessionId) {
        self.ended = true;
        self.place.finish(&self.mob_id, &self.member, Some(session));
    }
}

impl Drop for MemberSocketClaim {
    fn drop(&mut self) {
        if !self.ended {
            self.place.finish(&self.mob_id, &self.member, None);
        }
    }
}

/// Whether the holder's session still exists: live, or stored and not
/// archived. A read that fails keeps the holder.
async fn session_exists(service: &dyn MobSessionService, session: &SessionId) -> bool {
    match service.observe_session_resume_authority(session).await {
        Ok(authority) => match authority.lifecycle() {
            SessionResumeLifecycle::Archived { .. } => false,
            // A service without a durable record of the session answers
            // from its live sessions.
            SessionResumeLifecycle::NoCurrentDurableAuthority => service
                .live_session_actor_registered(session)
                .await
                .unwrap_or(true),
            _ => true,
        },
        Err(_) => true,
    }
}

/// What a refused member's caller reads. It names the refused member and
/// nothing of the member that holds the place: that member may be one the
/// caller cannot see (a member of another mob, a member private to another
/// person).
pub(crate) fn refusal_text(member: &str) -> String {
    format!(
        "member '{}' cannot be created: this gateway runs with the member socket option \
         (member_comms_address), which serves one member per gateway, and another member of \
         this gateway already holds its comms key. Retire that member first, or run the \
         members without the option.",
        crate::member_comms_id::runtime_alias_str(member),
    )
}

/// The warning a gateway logs once at its start when the option is on and
/// its roster declares more than one member.
pub(crate) const SEVERAL_DECLARED_MEMBERS_WARNING: &str = "the member socket option \
     (member_comms_address) serves one member per gateway; the first member built holds it and \
     every other declared member is refused when it is built";

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn member(mob: &str, member: &str) -> MobMemberBinding {
        MobMemberBinding {
            mob_id: mob.to_string(),
            role: "worker".to_string(),
            member: member.to_string(),
        }
    }

    /// A refused member may be asked for again without end. The holder is
    /// written to the log once for each refused member, for a bounded number
    /// of them, and a new holder starts a new count.
    #[test]
    fn a_holder_is_logged_once_for_each_refused_member_up_to_a_cap() {
        let place = MemberSocketPlace::new();
        assert!(matches!(
            place.admit(&member("m", "holder")),
            Admission::Admitted
        ));
        let holder = place
            .place
            .lock()
            .expect("the place")
            .holder
            .clone()
            .expect("the holder");
        let logged = || place.place.lock().expect("the place").logged_refusals.len();

        for _ in 0..3 {
            let _ = place.refuse(&member("m", "carol"), &holder);
        }
        assert_eq!(logged(), 1, "one line for a member refused three times");
        for index in 0..2 * LOGGED_REFUSALS_PER_HOLDER {
            let _ = place.refuse(&member("m", &format!("member-{index}")), &holder);
        }
        assert_eq!(logged(), LOGGED_REFUSALS_PER_HOLDER, "the log is bounded");

        // The holder's create fails: the place is free, and the next holder
        // is logged for its own refusals.
        place.finish("m", "holder", None);
        assert_eq!(logged(), 0);
        assert!(matches!(
            place.admit(&member("m", "next")),
            Admission::Admitted
        ));
    }

    /// The refusal names the refused member and nothing else of the
    /// composition: no other member, no mob, no session.
    #[test]
    fn the_refusal_names_the_refused_member_only() {
        let place = MemberSocketPlace::new();
        assert!(matches!(
            place.admit(&member("holders-mob", "holder-name")),
            Admission::Admitted
        ));
        let holder = place
            .place
            .lock()
            .expect("the place")
            .holder
            .clone()
            .expect("the holder");
        let refused = place.refuse(&member("another-mob", "carol"), &holder);
        assert_eq!(
            refused.to_string(),
            "member 'carol' cannot be created: this gateway runs with the member socket option \
             (member_comms_address), which serves one member per gateway, and another member of \
             this gateway already holds its comms key. Retire that member first, or run the \
             members without the option."
        );
        let shown = format!("{refused:?}");
        for fact in ["holder-name", "holders-mob", "another-mob"] {
            assert!(!shown.contains(fact), "{shown}");
        }
    }
}
