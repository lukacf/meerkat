//! Focused behavioural tests for the canonical `MobStoreRestoreLifecycleMachine`.
//!
//! The machine is record-scoped: every test drives ONE quarantined store
//! record. It reads no clock and never re-attempts on its own: the shell asks
//! for a re-attempt, explicitly or on demand, and reports how it ended. The
//! machine decides which requests it admits.
//!
//! [`every_input_in_every_phase_is_total_and_only_mutates_where_intended`]
//! drives every input against every phase and asserts the one intended
//! transition or a byte-identical refusal.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::mob_store_restore_lifecycle::{
    MobQuarantineClass, MobStoreRestoreLifecycleEffect, MobStoreRestoreLifecycleInput,
    MobStoreRestoreLifecycleMachineAuthority, MobStoreRestoreLifecycleMachineMutator,
    MobStoreRestoreLifecycleMachineTransition, MobStoreRestoreLifecyclePhase,
    MobStoreRestoreOutcome, MobStoreRestoreRefusal, MobStoreRetryTrigger,
};

type Authority = MobStoreRestoreLifecycleMachineAuthority;
type Input = MobStoreRestoreLifecycleInput;
type Effect = MobStoreRestoreLifecycleEffect;
type Phase = MobStoreRestoreLifecyclePhase;
type Transition = MobStoreRestoreLifecycleMachineTransition;
type Trigger = MobStoreRetryTrigger;

fn apply(authority: &mut Authority, input: Input) -> Transition {
    MobStoreRestoreLifecycleMachineMutator::apply(authority, input)
        .expect("every input resolves to an explicit typed transition")
}

fn only_effect(transition: &Transition) -> Effect {
    let effects = transition.effects().to_vec();
    assert_eq!(
        effects.len(),
        1,
        "every transition emits exactly one verdict, got {effects:?}"
    );
    effects[0].clone()
}

fn quarantine(class: MobQuarantineClass) -> Input {
    Input::Quarantine { class }
}

fn request_retry(trigger: Trigger) -> Input {
    Input::RequestRetry { trigger }
}

fn resolve(outcome: MobStoreRestoreOutcome, class: MobQuarantineClass) -> Input {
    Input::ResolveRetry { outcome, class }
}

/// Build an authority parked in `phase`, quarantined as unavailable.
fn authority_in(phase: Phase) -> Authority {
    authority_in_class(phase, MobQuarantineClass::StoreUnavailable)
}

/// Build an authority parked in `phase`, quarantined under `class`; a
/// re-attempt it needs is an explicit one.
fn authority_in_class(phase: Phase, class: MobQuarantineClass) -> Authority {
    let mut authority = Authority::new();
    match phase {
        Phase::Unclassified => {}
        Phase::Quarantined => {
            apply(&mut authority, quarantine(class));
        }
        Phase::Retrying => {
            apply(&mut authority, quarantine(class));
            apply(&mut authority, request_retry(Trigger::Explicit));
        }
        Phase::Released => {
            apply(&mut authority, quarantine(class));
            apply(&mut authority, request_retry(Trigger::Explicit));
            apply(
                &mut authority,
                resolve(MobStoreRestoreOutcome::Managed, class),
            );
        }
    }
    assert_eq!(authority.state().lifecycle_phase, phase);
    authority
}

const ALL_PHASES: [Phase; 4] = [
    Phase::Unclassified,
    Phase::Quarantined,
    Phase::Retrying,
    Phase::Released,
];

const ALL_CLASSES: [MobQuarantineClass; 3] = [
    MobQuarantineClass::StoreUnavailable,
    MobQuarantineClass::StoreUnreadable,
    MobQuarantineClass::ResumeRefused,
];

const ALL_TRIGGERS: [Trigger; 2] = [Trigger::Explicit, Trigger::OnDemand];

fn assert_refused(transition: &Transition, reason: MobStoreRestoreRefusal) {
    assert!(
        matches!(
            only_effect(transition),
            Effect::RequestRefused { reason: refused } if refused == reason
        ),
        "expected the typed refusal {reason:?}, got {:?}",
        transition.effects()
    );
}

#[test]
fn a_pass_classifies_a_store_once() {
    let mut authority = Authority::new();
    assert_eq!(authority.state().lifecycle_phase, Phase::Unclassified);
    assert_eq!(authority.state().attempts, 0);
    let quarantined = apply(
        &mut authority,
        quarantine(MobQuarantineClass::StoreUnreadable),
    );
    assert!(matches!(
        only_effect(&quarantined),
        Effect::StoreQuarantined {
            class: MobQuarantineClass::StoreUnreadable
        }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Quarantined);
    assert_eq!(authority.state().class, MobQuarantineClass::StoreUnreadable);
    assert_eq!(authority.state().attempts, 0);

    for phase in [Phase::Quarantined, Phase::Retrying, Phase::Released] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let refused = apply(
            &mut authority,
            quarantine(MobQuarantineClass::ResumeRefused),
        );
        assert_refused(&refused, MobStoreRestoreRefusal::AlreadyClassified);
        assert_eq!(
            authority.state(),
            &before,
            "a second classification in {phase:?} changes nothing"
        );
    }
}

#[test]
fn one_re_attempt_is_in_flight_at_a_time() {
    for trigger in ALL_TRIGGERS {
        let mut authority = authority_in(Phase::Quarantined);
        let admitted = apply(&mut authority, request_retry(trigger));
        assert!(matches!(
            only_effect(&admitted),
            Effect::RetryAdmitted { attempts: 0 }
        ));
        assert_eq!(authority.state().lifecycle_phase, Phase::Retrying);

        // A second request while the first runs is refused at once, whatever
        // asks: it never waits for the attempt in flight, and changes
        // nothing.
        for second in ALL_TRIGGERS {
            let before = authority.state().clone();
            let refused = apply(&mut authority, request_retry(second));
            assert_refused(&refused, MobStoreRestoreRefusal::RetryInFlight);
            assert_eq!(authority.state(), &before);
        }

        for phase in [Phase::Unclassified, Phase::Released] {
            let mut authority = authority_in(phase);
            let before = authority.state().clone();
            let refused = apply(&mut authority, request_retry(trigger));
            assert_refused(&refused, MobStoreRestoreRefusal::NotQuarantined);
            assert_eq!(authority.state(), &before);
        }
    }
}

/// The machine decides which re-attempts it admits: one on demand (an
/// id-addressed verb that meets the quarantine) only for a store that could
/// not serve now, and an explicit one (for example after a repair) for a
/// store of any class.
#[test]
fn an_on_demand_re_attempt_is_admitted_only_for_a_store_that_could_not_serve() {
    for class in ALL_CLASSES {
        let mut authority = authority_in_class(Phase::Quarantined, class);
        let before = authority.state().clone();
        let on_demand = apply(&mut authority, request_retry(Trigger::OnDemand));
        if class == MobQuarantineClass::StoreUnavailable {
            assert!(
                matches!(
                    only_effect(&on_demand),
                    Effect::RetryAdmitted { attempts: 0 }
                ),
                "{class:?}: {:?}",
                on_demand.effects()
            );
            assert_eq!(authority.state().lifecycle_phase, Phase::Retrying);
        } else {
            assert_refused(&on_demand, MobStoreRestoreRefusal::NotRetryable);
            assert_eq!(
                authority.state(),
                &before,
                "{class:?}: a refused on-demand re-attempt changes nothing"
            );
        }

        let mut authority = authority_in_class(Phase::Quarantined, class);
        let explicit = apply(&mut authority, request_retry(Trigger::Explicit));
        assert!(
            matches!(
                only_effect(&explicit),
                Effect::RetryAdmitted { attempts: 0 }
            ),
            "{class:?}: {:?}",
            explicit.effects()
        );
        assert_eq!(authority.state().lifecycle_phase, Phase::Retrying);
        assert_eq!(authority.state().class, class);
    }
}

#[test]
fn a_re_attempt_releases_the_store_or_keeps_it_quarantined() {
    // A failed attempt keeps the store quarantined under the class it met,
    // and counts the attempt.
    let mut authority = authority_in(Phase::Retrying);
    let failed = apply(
        &mut authority,
        resolve(
            MobStoreRestoreOutcome::Failed,
            MobQuarantineClass::StoreUnreadable,
        ),
    );
    assert!(matches!(
        only_effect(&failed),
        Effect::StoreStillQuarantined {
            class: MobQuarantineClass::StoreUnreadable,
            attempts: 1
        }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Quarantined);
    assert_eq!(authority.state().class, MobQuarantineClass::StoreUnreadable);
    assert_eq!(authority.state().attempts, 1);

    // The next attempt is admitted again and releases the store.
    apply(&mut authority, request_retry(Trigger::Explicit));
    let released = apply(
        &mut authority,
        resolve(
            MobStoreRestoreOutcome::Managed,
            MobQuarantineClass::StoreUnreadable,
        ),
    );
    assert!(matches!(
        only_effect(&released),
        Effect::StoreReleased {
            outcome: MobStoreRestoreOutcome::Managed,
            attempts: 2
        }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Released);

    for outcome in [
        MobStoreRestoreOutcome::Held,
        MobStoreRestoreOutcome::Settled,
    ] {
        let mut authority = authority_in(Phase::Retrying);
        let released = apply(
            &mut authority,
            resolve(outcome, MobQuarantineClass::StoreUnavailable),
        );
        assert!(matches!(
            only_effect(&released),
            Effect::StoreReleased { outcome: ended, attempts: 1 } if ended == outcome
        ));
        assert_eq!(authority.state().lifecycle_phase, Phase::Released);
    }

    // An outcome with no attempt in flight is refused and changes nothing.
    for phase in [Phase::Unclassified, Phase::Quarantined, Phase::Released] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let refused = apply(
            &mut authority,
            resolve(
                MobStoreRestoreOutcome::Managed,
                MobQuarantineClass::StoreUnavailable,
            ),
        );
        assert_refused(&refused, MobStoreRestoreRefusal::NotRetrying);
        assert_eq!(authority.state(), &before);
    }
}

#[test]
fn every_input_in_every_phase_is_total_and_only_mutates_where_intended() {
    let inputs: [(&str, fn() -> Input); 5] = [
        ("Quarantine", || {
            quarantine(MobQuarantineClass::ResumeRefused)
        }),
        ("RequestRetryExplicit", || request_retry(Trigger::Explicit)),
        ("RequestRetryOnDemand", || request_retry(Trigger::OnDemand)),
        ("ResolveRetryFailed", || {
            resolve(
                MobStoreRestoreOutcome::Failed,
                MobQuarantineClass::StoreUnreadable,
            )
        }),
        ("ResolveRetryManaged", || {
            resolve(
                MobStoreRestoreOutcome::Managed,
                MobQuarantineClass::StoreUnavailable,
            )
        }),
    ];
    for class in ALL_CLASSES {
        for phase in ALL_PHASES {
            for (name, input) in inputs {
                let mut authority = authority_in_class(phase, class);
                let before = authority.state().clone();
                let transition = apply(&mut authority, input());
                let mutates = match (phase, name) {
                    (Phase::Unclassified, "Quarantine")
                    | (Phase::Quarantined, "RequestRetryExplicit")
                    | (Phase::Retrying, "ResolveRetryFailed")
                    | (Phase::Retrying, "ResolveRetryManaged") => true,
                    // On demand, only a store that could not serve now.
                    (Phase::Quarantined, "RequestRetryOnDemand") => {
                        class == MobQuarantineClass::StoreUnavailable
                    }
                    _ => false,
                };
                if mutates {
                    assert_ne!(
                        authority.state(),
                        &before,
                        "{name} in {phase:?} ({class:?}) is the one intended transition"
                    );
                } else {
                    assert_eq!(
                        authority.state(),
                        &before,
                        "{name} in {phase:?} ({class:?}) is a refusal that changes nothing"
                    );
                    assert!(
                        matches!(only_effect(&transition), Effect::RequestRefused { .. }),
                        "{name} in {phase:?} ({class:?}) answers a typed refusal"
                    );
                }
            }
        }
    }
}

#[test]
fn only_a_store_that_could_not_serve_is_retryable() {
    assert!(MobQuarantineClass::StoreUnavailable.is_retryable());
    assert!(!MobQuarantineClass::StoreUnreadable.is_retryable());
    assert!(!MobQuarantineClass::ResumeRefused.is_retryable());
    assert_eq!(
        MobQuarantineClass::StoreUnavailable.as_str(),
        "store_unavailable"
    );
    assert_eq!(
        MobQuarantineClass::StoreUnreadable.as_str(),
        "store_unreadable"
    );
    assert_eq!(MobQuarantineClass::ResumeRefused.as_str(), "resume_refused");
}
