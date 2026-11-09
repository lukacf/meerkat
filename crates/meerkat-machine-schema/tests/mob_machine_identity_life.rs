//! Rows for a MobMachine identity's life marker.
//!
//! The marker is the mob's resets over its whole journal and, per identity,
//! the ends of its lives since the last reset. It moves at every end of an
//! identity and at nothing else: `EndIdentityLife` (the end of a retirement
//! the shell reads as ending the identity, and the replay of its row) moves
//! the named identity's ends only, in every phase, and the resets are
//! recovered at resume only before the high-water seal. These rows drive the
//! schema crate's own instantiation of the machine through its signals.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::mob_machine::{
    AgentIdentity, MobMachineAuthority, MobMachineSignal, MobMachineTransition,
    MobMachineTransitionError,
};

type Authority = MobMachineAuthority;
type Signal = MobMachineSignal;

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

fn signal(authority: &mut Authority, signal: Signal) -> MobMachineTransition {
    authority
        .apply_signal(signal)
        .expect("the signal must resolve to exactly one arm")
}

fn ends(authority: &Authority, name: &str) -> Option<u64> {
    authority
        .state()
        .identity_life_ends
        .get(&identity(name))
        .copied()
}

/// The end of an identity's life moves that identity's marker by one at each
/// end, emits nothing, and leaves every other identity's marker and the
/// mob's resets as they are.
#[test]
fn an_identity_life_end_moves_only_that_identitys_marker() {
    let mut authority = Authority::new();
    assert_eq!(ends(&authority, "alpha"), None, "a fresh mob counts no end");
    assert_eq!(authority.state().identity_life_resets, 0);

    let first = signal(
        &mut authority,
        Signal::EndIdentityLife {
            agent_identity: identity("alpha"),
        },
    );
    assert!(
        first.effects().is_empty(),
        "the marker's move emits nothing"
    );
    assert_eq!(ends(&authority, "alpha"), Some(1));
    assert_eq!(
        ends(&authority, "beta"),
        None,
        "another identity keeps its marker"
    );

    signal(
        &mut authority,
        Signal::EndIdentityLife {
            agent_identity: identity("alpha"),
        },
    );
    assert_eq!(
        ends(&authority, "alpha"),
        Some(2),
        "each end moves it again"
    );
    assert_eq!(
        authority.state().identity_life_resets,
        0,
        "an identity's end moves no reset"
    );
}

/// Resume recovers the mob's resets once, before it seals the high-water
/// window; a sealed machine refuses the recovery and keeps its count.
#[test]
fn the_resets_are_recovered_only_before_the_seal() {
    let mut authority = Authority::new();
    signal(
        &mut authority,
        Signal::RecoverIdentityLifeResets { resets: 3 },
    );
    assert_eq!(authority.state().identity_life_resets, 3);

    signal(&mut authority, Signal::SealIdentityGenerationHighwater);
    match authority.apply_signal(Signal::RecoverIdentityLifeResets { resets: 4 }) {
        Err(
            MobMachineTransitionError::GuardRejected { .. }
            | MobMachineTransitionError::NoMatchingTransition { .. },
        ) => {}
        other => panic!("a sealed machine must refuse the recovery: {other:?}"),
    }
    assert_eq!(
        authority.state().identity_life_resets,
        3,
        "the refused recovery changes nothing"
    );
}
