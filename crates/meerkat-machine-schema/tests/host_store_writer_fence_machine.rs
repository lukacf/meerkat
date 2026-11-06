//! Focused behavioural tests for the canonical `HostStoreWriterFenceMachine`.
//!
//! The machine is realm-scoped: every test drives the writer fence of ONE
//! realm. It reads no clock and does no store I/O: the realm reports each
//! answer and each clock fact as an input, and the machine decides what the
//! writer does next.
//!
//! [`every_input_in_every_phase_is_total_and_only_mutates_where_intended`]
//! drives every input against every phase and asserts the intended
//! transition or a state-identical refusal.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::host_store_writer_fence::{
    HostStoreWriterFenceEffect, HostStoreWriterFenceInput, HostStoreWriterFenceMachineAuthority,
    HostStoreWriterFenceMachineMutator, HostStoreWriterFenceMachineTransition,
    HostStoreWriterFencePhase, WriterFenceAcquireOutcome, WriterFenceAcquireStep,
    WriterFenceListingOutcome, WriterFenceLostCause, WriterFenceRefusal, WriterFenceRenewOutcome,
    WriterFenceStopCause,
};

type Authority = HostStoreWriterFenceMachineAuthority;
type Input = HostStoreWriterFenceInput;
type Effect = HostStoreWriterFenceEffect;
type Phase = HostStoreWriterFencePhase;
type Transition = HostStoreWriterFenceMachineTransition;

/// The open's token in every test.
const OPENED: u64 = 1;
/// The grant of the first re-acquire.
const GRANT: u64 = 2;
/// A holder the writer does not know.
const STRANGER: u64 = 7;

fn apply(authority: &mut Authority, input: Input) -> Transition {
    HostStoreWriterFenceMachineMutator::apply(authority, input)
        .expect("every input resolves to an explicit typed transition")
}

fn effects(transition: &Transition) -> Vec<Effect> {
    transition.effects().to_vec()
}

fn only_effect(transition: &Transition) -> Effect {
    let effects = effects(transition);
    assert_eq!(
        effects.len(),
        1,
        "the transition emits exactly one effect, got {effects:?}"
    );
    effects[0].clone()
}

fn renew(outcome: WriterFenceRenewOutcome, renewed: u64, current: u64, in_window: bool) -> Input {
    Input::RenewAnswered {
        outcome,
        renewed,
        current,
        in_window,
    }
}

fn renewed_in_window(token: u64) -> Input {
    renew(WriterFenceRenewOutcome::Renewed, token, token, true)
}

fn acquire(outcome: WriterFenceAcquireOutcome, token: u64) -> Input {
    Input::AcquireAnswered { outcome, token }
}

fn listing(outcome: WriterFenceListingOutcome, token: u64) -> Input {
    Input::ListingChecked { outcome, token }
}

/// Build an authority parked in `phase`: opened under token 1, stopped by a
/// closed window, re-acquiring, proving or resuming on the grant of token 2,
/// waiting out the holder 7 after a timed-out acquire, or lost to an
/// adapter rule.
fn authority_in(phase: Phase) -> Authority {
    let mut authority = Authority::new();
    match phase {
        Phase::Unopened => {}
        Phase::Live => {
            apply(&mut authority, Input::Open { token: OPENED });
        }
        Phase::Stopped => {
            apply(&mut authority, Input::Open { token: OPENED });
            apply(&mut authority, Input::WindowClosed);
        }
        Phase::Acquiring => {
            authority = authority_in(Phase::Stopped);
            apply(&mut authority, Input::BeginReacquire);
        }
        Phase::Proving => {
            authority = authority_in(Phase::Acquiring);
            apply(
                &mut authority,
                acquire(WriterFenceAcquireOutcome::Granted, GRANT),
            );
        }
        Phase::Resuming => {
            authority = authority_in(Phase::Proving);
            apply(
                &mut authority,
                listing(WriterFenceListingOutcome::Clean, GRANT),
            );
        }
        Phase::WaitingOut => {
            authority = authority_in(Phase::Acquiring);
            apply(
                &mut authority,
                acquire(WriterFenceAcquireOutcome::TimedOut, 0),
            );
            apply(&mut authority, Input::BeginReacquire);
            apply(
                &mut authority,
                acquire(WriterFenceAcquireOutcome::Held, STRANGER),
            );
        }
        Phase::Lost => {
            authority = authority_in(Phase::Live);
            apply(
                &mut authority,
                Input::Poison {
                    cause: WriterFenceLostCause::AdapterRule,
                },
            );
        }
    }
    assert_eq!(authority.state().lifecycle_phase, phase);
    authority
}

const ALL_PHASES: [Phase; 8] = [
    Phase::Unopened,
    Phase::Live,
    Phase::Stopped,
    Phase::Acquiring,
    Phase::Proving,
    Phase::Resuming,
    Phase::WaitingOut,
    Phase::Lost,
];

fn assert_refused(transition: &Transition, reason: WriterFenceRefusal) {
    assert!(
        matches!(
            only_effect(transition),
            Effect::RequestRefused { reason: refused } if refused == reason
        ),
        "expected the typed refusal {reason:?}, got {:?}",
        transition.effects()
    );
}

fn assert_stopped_by(transition: &Transition, cause: WriterFenceStopCause) {
    assert!(
        matches!(
            only_effect(transition),
            Effect::WriterStopped { cause: stopped } if stopped == cause
        ),
        "expected the writer stopped by {cause:?}, got {:?}",
        transition.effects()
    );
}

fn assert_lost_to(transition: &Transition, cause: WriterFenceLostCause) {
    assert!(
        matches!(
            only_effect(transition),
            Effect::RealmLost { cause: lost } if lost == cause
        ),
        "expected the realm lost to {cause:?}, got {:?}",
        transition.effects()
    );
}

#[test]
fn an_open_installs_its_grant_once() {
    let mut authority = Authority::new();
    assert_eq!(authority.state().lifecycle_phase, Phase::Unopened);
    assert_eq!(authority.state().token, 0);
    assert!(!authority.state().writer_stopped);

    let opened = apply(&mut authority, Input::Open { token: OPENED });
    assert!(matches!(
        only_effect(&opened),
        Effect::WriterOpened { token: OPENED }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Live);
    assert_eq!(authority.state().token, OPENED);

    // The open's grant is the first acquire: any token the store answered.
    let mut zero = Authority::new();
    apply(&mut zero, Input::Open { token: 0 });
    assert_eq!(zero.state().lifecycle_phase, Phase::Live);
    assert_eq!(zero.state().token, 0);

    for phase in ALL_PHASES
        .into_iter()
        .filter(|phase| *phase != Phase::Unopened)
    {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let refused = apply(&mut authority, Input::Open { token: 9 });
        let reason = if phase == Phase::Lost {
            WriterFenceRefusal::Lost
        } else {
            WriterFenceRefusal::AlreadyOpened
        };
        assert_refused(&refused, reason);
        assert_eq!(authority.state(), &before, "{phase:?}");
    }
}

#[test]
fn a_live_writer_stops_on_a_failed_renew_or_a_closed_window() {
    // A renew that started inside the window anchors it.
    let mut authority = authority_in(Phase::Live);
    let anchored = apply(&mut authority, renewed_in_window(OPENED));
    assert!(matches!(
        only_effect(&anchored),
        Effect::WindowAnchored { token: OPENED }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Live);

    // Every other renew answer that is not a fence-out stops the writer.
    for (input, cause) in [
        (
            renew(WriterFenceRenewOutcome::Renewed, OPENED, OPENED, false),
            WriterFenceStopCause::WindowClosedBeforeRenew,
        ),
        (
            renew(WriterFenceRenewOutcome::Failed, OPENED, OPENED, true),
            WriterFenceStopCause::RenewFailed,
        ),
        (
            renew(WriterFenceRenewOutcome::TimedOut, OPENED, OPENED, true),
            WriterFenceStopCause::RenewTimedOut,
        ),
        (Input::WindowClosed, WriterFenceStopCause::WindowClosed),
    ] {
        let mut authority = authority_in(Phase::Live);
        let stopped = apply(&mut authority, input);
        assert_stopped_by(&stopped, cause);
        assert_eq!(authority.state().lifecycle_phase, Phase::Stopped);
        assert_eq!(authority.state().stop_cause, cause);
        assert_eq!(authority.state().token, OPENED);
        assert!(authority.state().writer_stopped);
    }

    // A writer that is not live writes nothing already: a closed window
    // changes nothing and says nothing.
    for phase in [
        Phase::Stopped,
        Phase::Acquiring,
        Phase::Proving,
        Phase::Resuming,
        Phase::WaitingOut,
    ] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let closed = apply(&mut authority, Input::WindowClosed);
        assert!(
            effects(&closed).is_empty(),
            "{phase:?}: {:?}",
            effects(&closed)
        );
        assert_eq!(authority.state(), &before, "{phase:?}");
    }
    for (phase, reason) in [
        (Phase::Unopened, WriterFenceRefusal::NotOpened),
        (Phase::Lost, WriterFenceRefusal::Lost),
    ] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        assert_refused(&apply(&mut authority, Input::WindowClosed), reason);
        assert_eq!(authority.state(), &before);
    }
}

#[test]
fn renew_answers_never_resume_a_stopped_writer() {
    // A renew that answers once the writer stopped moves nothing.
    for outcome in [
        WriterFenceRenewOutcome::Renewed,
        WriterFenceRenewOutcome::Failed,
        WriterFenceRenewOutcome::TimedOut,
    ] {
        let mut authority = authority_in(Phase::Stopped);
        let before = authority.state().clone();
        let ignored = apply(&mut authority, renew(outcome, OPENED, OPENED, true));
        assert!(matches!(only_effect(&ignored), Effect::RenewIgnored));
        assert_eq!(authority.state(), &before);
    }

    // A fence-out naming the writer's own token, above the renewed one, is a
    // stale renew of a token the writer superseded itself.
    for phase in [Phase::Live, Phase::Stopped] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let ignored = apply(
            &mut authority,
            renew(WriterFenceRenewOutcome::FencedOut, 0, OPENED, true),
        );
        assert!(matches!(only_effect(&ignored), Effect::RenewIgnored));
        assert_eq!(authority.state(), &before);
    }

    // Any other fence-out loses the realm: one naming the renewed token
    // itself, or a token this writer does not hold.
    for phase in [Phase::Live, Phase::Stopped] {
        for current in [OPENED, STRANGER] {
            let mut authority = authority_in(phase);
            let lost = apply(
                &mut authority,
                renew(WriterFenceRenewOutcome::FencedOut, OPENED, current, true),
            );
            assert_lost_to(&lost, WriterFenceLostCause::FencedOutRenew);
            assert_eq!(authority.state().lifecycle_phase, Phase::Lost);
        }
    }

    // No renew can be in flight before the open, during a re-acquire's
    // acquire or listing, or while a holder is waited out.
    for phase in [
        Phase::Unopened,
        Phase::Acquiring,
        Phase::Proving,
        Phase::WaitingOut,
    ] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let refused = apply(&mut authority, renewed_in_window(OPENED));
        assert_refused(&refused, WriterFenceRefusal::NoRenewInFlight);
        assert_eq!(authority.state(), &before, "{phase:?}");
    }
    let mut authority = authority_in(Phase::Lost);
    let before = authority.state().clone();
    assert_refused(
        &apply(&mut authority, renewed_in_window(OPENED)),
        WriterFenceRefusal::Lost,
    );
    assert_eq!(authority.state(), &before);
}

#[test]
fn a_stopped_writer_resumes_only_through_a_proved_grant() {
    let mut authority = authority_in(Phase::Stopped);
    let begun = apply(&mut authority, Input::BeginReacquire);
    assert!(matches!(
        only_effect(&begun),
        Effect::AcquireRequested {
            step: WriterFenceAcquireStep::Exclusive
        }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Acquiring);

    // Its own recorded lease answers held: take it over.
    let takeover = apply(
        &mut authority,
        acquire(WriterFenceAcquireOutcome::Held, OPENED),
    );
    assert!(matches!(
        only_effect(&takeover),
        Effect::AcquireRequested {
            step: WriterFenceAcquireStep::TakeoverOwn
        }
    ));
    assert_eq!(authority.state().step, WriterFenceAcquireStep::TakeoverOwn);

    let granted = apply(
        &mut authority,
        acquire(WriterFenceAcquireOutcome::Granted, GRANT),
    );
    assert!(matches!(
        only_effect(&granted),
        Effect::GrantReceived { token: GRANT }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Proving);
    assert_eq!(authority.state().granted, Some(GRANT));
    assert_eq!(authority.state().own_grant, Some(GRANT));
    assert_eq!(
        authority.state().token,
        OPENED,
        "no token is installed before the listing proves the grant"
    );

    let installed = apply(
        &mut authority,
        listing(WriterFenceListingOutcome::Clean, GRANT),
    );
    assert!(matches!(
        only_effect(&installed),
        Effect::TokenInstalled { token: GRANT }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Resuming);
    assert_eq!(authority.state().token, GRANT);
    assert_eq!(authority.state().own_grant, None);

    let resumed = apply(&mut authority, renewed_in_window(GRANT));
    assert!(matches!(
        only_effect(&resumed),
        Effect::WriterResumed { token: GRANT }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Live);
    assert_eq!(authority.state().granted, None);
    assert!(!authority.state().writer_stopped);

    // One attempt at a time, from Stopped only.
    for (phase, reason) in [
        (Phase::Live, WriterFenceRefusal::AlreadyLive),
        (Phase::Acquiring, WriterFenceRefusal::AttemptInFlight),
        (Phase::Proving, WriterFenceRefusal::AttemptInFlight),
        (Phase::Resuming, WriterFenceRefusal::AttemptInFlight),
        (Phase::Unopened, WriterFenceRefusal::NotOpened),
        (Phase::Lost, WriterFenceRefusal::Lost),
    ] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        assert_refused(&apply(&mut authority, Input::BeginReacquire), reason);
        assert_eq!(authority.state(), &before, "{phase:?}");
    }
}

#[test]
fn a_failed_attempt_leaves_the_writer_stopped() {
    for (input, cause) in [
        (
            acquire(WriterFenceAcquireOutcome::Failed, 0),
            WriterFenceStopCause::ReacquireFailed,
        ),
        (
            acquire(WriterFenceAcquireOutcome::TimedOut, 0),
            WriterFenceStopCause::ReacquireTimedOut,
        ),
    ] {
        let mut authority = authority_in(Phase::Acquiring);
        assert_stopped_by(&apply(&mut authority, input), cause);
        assert_eq!(authority.state().lifecycle_phase, Phase::Stopped);
        let timed_out = u64::from(cause == WriterFenceStopCause::ReacquireTimedOut);
        assert_eq!(authority.state().timed_out, timed_out);
    }

    // A takeover answered held failed.
    let mut authority = authority_in(Phase::Acquiring);
    apply(
        &mut authority,
        acquire(WriterFenceAcquireOutcome::Held, OPENED),
    );
    assert_stopped_by(
        &apply(
            &mut authority,
            acquire(WriterFenceAcquireOutcome::Held, OPENED),
        ),
        WriterFenceStopCause::ReacquireFailed,
    );

    // A resume renew that fails leaves the token installed.
    for (outcome, cause) in [
        (
            WriterFenceRenewOutcome::Failed,
            WriterFenceStopCause::ResumeRenewFailed,
        ),
        (
            WriterFenceRenewOutcome::TimedOut,
            WriterFenceStopCause::ResumeRenewTimedOut,
        ),
    ] {
        let mut authority = authority_in(Phase::Resuming);
        assert_stopped_by(
            &apply(&mut authority, renew(outcome, GRANT, GRANT, true)),
            cause,
        );
        assert_eq!(authority.state().token, GRANT);
        assert_eq!(authority.state().granted, None);
    }

    // A listing that fails keeps the grant as the writer's own lease, and
    // the next attempt takes it over.
    let mut authority = authority_in(Phase::Proving);
    assert_stopped_by(
        &apply(
            &mut authority,
            listing(WriterFenceListingOutcome::Failed, GRANT),
        ),
        WriterFenceStopCause::ListingFailed,
    );
    assert_eq!(authority.state().token, OPENED);
    assert_eq!(authority.state().granted, None);
    assert_eq!(authority.state().own_grant, Some(GRANT));
    apply(&mut authority, Input::BeginReacquire);
    let takeover = apply(
        &mut authority,
        acquire(WriterFenceAcquireOutcome::Held, GRANT),
    );
    assert!(matches!(
        only_effect(&takeover),
        Effect::AcquireRequested {
            step: WriterFenceAcquireStep::TakeoverOwn
        }
    ));
    apply(
        &mut authority,
        acquire(WriterFenceAcquireOutcome::Granted, GRANT + 1),
    );
    assert_eq!(authority.state().own_grant, Some(GRANT + 1));
    apply(
        &mut authority,
        listing(WriterFenceListingOutcome::Clean, GRANT + 1),
    );
    apply(&mut authority, renewed_in_window(GRANT + 1));
    assert_eq!(authority.state().lifecycle_phase, Phase::Live);
    assert_eq!(authority.state().token, GRANT + 1);
}

#[test]
fn an_unknown_holder_is_waited_out_only_after_a_timed_out_acquire() {
    // No acquire timed out since the last grant: another writer.
    let mut authority = authority_in(Phase::Acquiring);
    assert_lost_to(
        &apply(
            &mut authority,
            acquire(WriterFenceAcquireOutcome::Held, STRANGER),
        ),
        WriterFenceLostCause::HeldByAnother,
    );

    // An acquire timed out: the holder may be its own late grant.
    let mut authority = authority_in(Phase::WaitingOut);
    assert_eq!(authority.state().waited_holder, Some(STRANGER));
    assert_eq!(authority.state().timed_out, 1);
    let before = authority.state().clone();
    assert_refused(
        &apply(&mut authority, Input::BeginReacquire),
        WriterFenceRefusal::WaitingOutHolder,
    );
    assert_eq!(authority.state(), &before);

    // Its recorded lease ended: acquire again.
    let ended = apply(&mut authority, Input::LeaseEnded);
    assert!(effects(&ended).is_empty());
    assert_eq!(authority.state().lifecycle_phase, Phase::Stopped);
    assert_eq!(authority.state().waited_holder, Some(STRANGER));

    /// A writer that waited out the holder 7 and acquires again.
    fn acquiring_after_the_wait() -> Authority {
        let mut authority = authority_in(Phase::WaitingOut);
        apply(&mut authority, Input::LeaseEnded);
        apply(&mut authority, Input::BeginReacquire);
        authority
    }

    // The same holder still holds the fence: it renewed its lease.
    let mut renewed = acquiring_after_the_wait();
    assert_lost_to(
        &apply(
            &mut renewed,
            acquire(WriterFenceAcquireOutcome::Held, STRANGER),
        ),
        WriterFenceLostCause::HolderRenewed,
    );

    // Another unknown holder is waited out in turn.
    let mut other = acquiring_after_the_wait();
    let waited = apply(
        &mut other,
        acquire(WriterFenceAcquireOutcome::Held, STRANGER + 1),
    );
    assert!(matches!(
        only_effect(&waited),
        Effect::HolderWaitedOut { holder } if holder == STRANGER + 1
    ));
    assert_eq!(other.state().lifecycle_phase, Phase::WaitingOut);

    // A grant ends every wait.
    let mut authority = acquiring_after_the_wait();
    let granted = apply(
        &mut authority,
        acquire(WriterFenceAcquireOutcome::Granted, STRANGER + 1),
    );
    assert!(matches!(
        only_effect(&granted),
        Effect::GrantReceived { .. }
    ));
    assert_eq!(authority.state().timed_out, 0);
    assert_eq!(authority.state().waited_holder, None);

    for phase in [
        Phase::Unopened,
        Phase::Live,
        Phase::Stopped,
        Phase::Acquiring,
        Phase::Proving,
        Phase::Resuming,
    ] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        assert_refused(
            &apply(&mut authority, Input::LeaseEnded),
            WriterFenceRefusal::NotWaitingOut,
        );
        assert_eq!(authority.state(), &before, "{phase:?}");
    }
    let mut authority = authority_in(Phase::Lost);
    assert_refused(
        &apply(&mut authority, Input::LeaseEnded),
        WriterFenceRefusal::Lost,
    );
}

#[test]
fn the_realm_is_lost_once_and_for_good() {
    let mut foreign = authority_in(Phase::Proving);
    assert_lost_to(
        &apply(
            &mut foreign,
            listing(WriterFenceListingOutcome::ForeignBatch, GRANT),
        ),
        WriterFenceLostCause::ForeignBatch,
    );
    assert_eq!(foreign.state().granted, None);

    let mut resumed = authority_in(Phase::Resuming);
    assert_lost_to(
        &apply(
            &mut resumed,
            renew(WriterFenceRenewOutcome::FencedOut, GRANT, STRANGER, true),
        ),
        WriterFenceLostCause::FencedOutAfterReacquire,
    );

    // A grant at or below the installed token breaks the store's token
    // order.
    for token in [0, OPENED] {
        let mut authority = authority_in(Phase::Acquiring);
        assert_lost_to(
            &apply(
                &mut authority,
                acquire(WriterFenceAcquireOutcome::Granted, token),
            ),
            WriterFenceLostCause::TokenOrder,
        );
    }

    // A poison loses the realm from every opened phase, and the first cause
    // stands.
    for phase in [
        Phase::Live,
        Phase::Stopped,
        Phase::Acquiring,
        Phase::Proving,
        Phase::Resuming,
        Phase::WaitingOut,
    ] {
        let mut authority = authority_in(phase);
        assert_lost_to(
            &apply(
                &mut authority,
                Input::Poison {
                    cause: WriterFenceLostCause::FencedOutCommit,
                },
            ),
            WriterFenceLostCause::FencedOutCommit,
        );
        assert_eq!(authority.state().granted, None);
        assert_eq!(
            authority.state().writer_stopped,
            phase != Phase::Live,
            "a loss keeps whether the writer had stopped"
        );
        let before = authority.state().clone();
        assert_refused(
            &apply(
                &mut authority,
                Input::Poison {
                    cause: WriterFenceLostCause::AdapterRule,
                },
            ),
            WriterFenceRefusal::Lost,
        );
        assert_eq!(authority.state(), &before);
        assert_eq!(
            authority.state().lost_cause,
            WriterFenceLostCause::FencedOutCommit
        );
    }
    let mut authority = authority_in(Phase::Unopened);
    assert_refused(
        &apply(
            &mut authority,
            Input::Poison {
                cause: WriterFenceLostCause::AdapterRule,
            },
        ),
        WriterFenceRefusal::NotOpened,
    );

    // Answers the writer did not ask for are refused.
    let mut proving = authority_in(Phase::Proving);
    let before = proving.state().clone();
    assert_refused(
        &apply(
            &mut proving,
            listing(WriterFenceListingOutcome::Clean, STRANGER),
        ),
        WriterFenceRefusal::NotProving,
    );
    assert_eq!(proving.state(), &before);
    for phase in ALL_PHASES {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        if phase != Phase::Proving {
            let refused = apply(
                &mut authority,
                listing(WriterFenceListingOutcome::Clean, GRANT),
            );
            let reason = if phase == Phase::Lost {
                WriterFenceRefusal::Lost
            } else {
                WriterFenceRefusal::NotProving
            };
            assert_refused(&refused, reason);
        }
        if phase != Phase::Acquiring {
            let refused = apply(
                &mut authority,
                acquire(WriterFenceAcquireOutcome::Granted, GRANT),
            );
            let reason = if phase == Phase::Lost {
                WriterFenceRefusal::Lost
            } else {
                WriterFenceRefusal::NotAcquiring
            };
            assert_refused(&refused, reason);
        }
        assert_eq!(authority.state(), &before, "{phase:?}");
    }
}

#[test]
fn every_input_in_every_phase_is_total_and_only_mutates_where_intended() {
    let inputs: [(&str, fn() -> Input); 11] = [
        ("Open", || Input::Open { token: 9 }),
        ("WindowClosed", || Input::WindowClosed),
        ("Renewed", || renewed_in_window(OPENED)),
        ("RenewFailed", || {
            renew(WriterFenceRenewOutcome::Failed, OPENED, OPENED, true)
        }),
        ("BeginReacquire", || Input::BeginReacquire),
        ("Granted", || acquire(WriterFenceAcquireOutcome::Granted, 5)),
        ("TimedOut", || {
            acquire(WriterFenceAcquireOutcome::TimedOut, 0)
        }),
        ("LeaseEnded", || Input::LeaseEnded),
        ("ListingClean", || {
            listing(WriterFenceListingOutcome::Clean, GRANT)
        }),
        ("ListingFailed", || {
            listing(WriterFenceListingOutcome::Failed, GRANT)
        }),
        ("Poison", || Input::Poison {
            cause: WriterFenceLostCause::AdapterRule,
        }),
    ];
    for phase in ALL_PHASES {
        for (name, input) in inputs {
            let mut authority = authority_in(phase);
            let before = authority.state().clone();
            let transition = apply(&mut authority, input());
            let mutates = match (phase, name) {
                (Phase::Unopened, "Open")
                | (Phase::Live, "WindowClosed" | "RenewFailed" | "Poison")
                | (Phase::Stopped, "BeginReacquire" | "Poison")
                | (Phase::Acquiring, "Granted" | "TimedOut" | "Poison")
                | (Phase::Proving, "ListingClean" | "ListingFailed" | "Poison")
                | (Phase::Resuming, "Renewed" | "RenewFailed" | "Poison")
                | (Phase::WaitingOut, "LeaseEnded" | "Poison") => true,
                _ => false,
            };
            if mutates {
                assert_ne!(
                    authority.state(),
                    &before,
                    "{phase:?} + {name} must move the writer"
                );
            } else {
                assert_eq!(
                    authority.state(),
                    &before,
                    "{phase:?} + {name} must change nothing, effects {:?}",
                    transition.effects()
                );
            }
        }
    }
}
