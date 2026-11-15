//! The lifecycle of a mob as its handles observe it without asking its actor.

use std::sync::Arc;

use crate::machines::mob_machine::{
    MobMachineState, MobPhase, PlacedCompletionLifecycleIntentKind,
};
use crate::{MobError, MobState};

/// The lifecycle of a mob as its handles observe it without asking its
/// actor: the last phase the actor published, the lifecycle intent the mob
/// still owes and, once the actor has stopped, why.
///
/// [`MobHandle::status`](super::MobHandle::status) asks the actor, so it
/// waits behind the actor's work. An observation reads only what the actor
/// published, so observation surfaces (listings, consoles, address
/// observation) stay live while the actor is busy and still see that an
/// actor has stopped. The phase alone is
/// [`MobHandle::status_observation_snapshot`](super::MobHandle::status_observation_snapshot).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MobLifecycleObservation {
    /// The last lifecycle phase the mob's actor published: the machine
    /// phase, not whether the actor is alive.
    pub phase: MobState,
    /// Present once the mob's actor has stopped, so it admits no command
    /// until a restart recovers the mob from its durable state:
    /// - the cause the actor recorded when it fail-stopped (its startup
    ///   recovery failed, or a durable mutation's outcome stayed uncertain),
    ///   which [`MobHandle::status`](super::MobHandle::status) reports in
    ///   [`MobError::SharedLifecycleFailure`];
    /// - for an actor task that ended while `phase` was still live
    ///   (`Creating` or `Running`) and recorded no cause, the closed-actor
    ///   error that `status` reports.
    pub failure: Option<Arc<MobError>>,
    /// The lifecycle intent the mob still owes, read from the mob machine's
    /// published state: one that was durably begun (a Stop, a Complete, a
    /// Reset, a retirement of every member) or admitted (a Destroy) and has
    /// not landed yet. `None` when that state owes nothing: no intent was
    /// begun, or it has landed, so a Stopped mob owes no Stop.
    ///
    /// While an intent is owed, `phase` is not yet the phase that intent
    /// leaves (an admitted Destroy excepted: its public phase is Destroyed
    /// from its admission): a host that acts on the phase (for example by
    /// lifting a Stopped mob) waits with
    /// [`MobHandle::wait_until_no_lifecycle_intent_is_owed`](super::MobHandle::wait_until_no_lifecycle_intent_is_owed)
    /// first. This field and `phase` are two reads, so a transition between
    /// them can show an owed Stop beside a Stopped phase; that wait returns
    /// the phase of the same state that owes nothing.
    ///
    /// The answer is the published machine state's, and in two places that
    /// state reads `None` while the mob's durable log still owes an intent:
    /// - Inside a pre-activation hook
    ///   ([`MobBuilder::before_activation`](crate::MobBuilder::before_activation))
    ///   of a resume. The intent the log leaves open (a Stop, Complete,
    ///   Reset or retirement of every member an earlier process began, or a
    ///   Destroy not yet admitted) is applied to the machine after the hook
    ///   has run, so the hook's read handle reads `None` for it; an admitted
    ///   destroy is already there. Read the owed intent from the handle the
    ///   resume returns.
    /// - After a graceful Shutdown over an owed Stop. The Shutdown leaves
    ///   this ended actor's machine Stopped, the state a landed Stop leaves,
    ///   so its handles read Stopped with nothing owed. The Stop has not
    ///   landed durably: the next resume comes back Running with the Stop
    ///   owed, and drives it.
    ///
    /// A match on the kind should keep a wildcard arm: a later release may
    /// add a kind of lifecycle intent.
    pub owed_intent: Option<PlacedCompletionLifecycleIntentKind>,
}

impl MobLifecycleObservation {
    pub(super) fn new(
        phase: MobState,
        failure: Option<Arc<MobError>>,
        owed_intent: Option<PlacedCompletionLifecycleIntentKind>,
    ) -> Self {
        Self {
            phase,
            failure,
            owed_intent,
        }
    }
}

/// The lifecycle intent a mob in this machine state still owes: the one rule
/// behind [`MobLifecycleObservation::owed_intent`], the wait on it, and the
/// recovery driver a resume starts.
///
/// It is a function of three machine facts and stores nothing: the phase,
/// the open lifecycle intent, and whether a destroy was admitted. The
/// machine keeps a Stop or Complete intent open in the phase it leaves, so
/// an open intent alone does not mean it is owed.
pub(super) fn owed_lifecycle_intent(
    state: &MobMachineState,
) -> Option<PlacedCompletionLifecycleIntentKind> {
    match (
        state.lifecycle_phase,
        state.placed_completion_lifecycle_intent,
    ) {
        // A destroyed mob owes nothing.
        (MobPhase::Destroyed, _) => None,
        // An admitted destroy is owed until the mob is Destroyed. The
        // machine opens the Destroy intent before it admits a destroy; the
        // admission alone decides here, as it does for the destroy retry of
        // a resume.
        _ if state.destroy_admitted => Some(PlacedCompletionLifecycleIntentKind::Destroy),
        // The intent has landed: the phase is the one it leaves.
        (MobPhase::Stopped, Some(PlacedCompletionLifecycleIntentKind::Stop))
        | (MobPhase::Completed, Some(PlacedCompletionLifecycleIntentKind::Complete)) => None,
        (_, intent) => intent,
    }
}

/// The public phase of a machine state that owes no lifecycle intent.
pub(super) fn phase_owing_no_lifecycle_intent(state: &MobMachineState) -> MobState {
    match state.lifecycle_phase {
        MobPhase::Running => MobState::Running,
        MobPhase::Stopped => MobState::Stopped,
        MobPhase::Completed => MobState::Completed,
        MobPhase::Destroyed => MobState::Destroyed,
    }
}
