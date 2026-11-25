//! Focused behavioural tests for the canonical `MobHostedSetLifecycleMachine`.
//!
//! The machine is set-scoped: every test drives the ONE state of a host's
//! mob set. It reads no clock and begins nothing on its own: the shell says
//! whether the host has stores from before, asks to begin the pass over
//! them, reports that it finished, and reports a host's declaration and a
//! host's own handle. The machine decides which of these it admits and
//! keeps the one answer the set is read for: whether every mob is known.
//!
//! [`every_reachable_state_answers_every_input_and_no_latch_is_ever_cleared`]
//! walks every state the inputs can reach and applies every input in each.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::mob_hosted_set_lifecycle::{
    MobHostedSetLifecycleEffect, MobHostedSetLifecycleInput, MobHostedSetLifecycleMachineAuthority,
    MobHostedSetLifecycleMachineMutator, MobHostedSetLifecycleMachineState,
    MobHostedSetLifecycleMachineTransition, MobHostedSetLifecyclePhase, MobHostedSetRefusal,
};

type Authority = MobHostedSetLifecycleMachineAuthority;
type Input = MobHostedSetLifecycleInput;
type Effect = MobHostedSetLifecycleEffect;
type Phase = MobHostedSetLifecyclePhase;
type Refusal = MobHostedSetRefusal;
type State = MobHostedSetLifecycleMachineState;
type Transition = MobHostedSetLifecycleMachineTransition;

fn apply(authority: &mut Authority, input: Input) -> Transition {
    MobHostedSetLifecycleMachineMutator::apply(authority, input)
        .expect("every input resolves to an explicit typed transition")
}

fn effects(transition: &Transition) -> Vec<Effect> {
    transition.effects().to_vec()
}

fn refused(transition: &Transition) -> Option<Refusal> {
    match effects(transition).as_slice() {
        [Effect::RequestRefused { reason }] => Some(*reason),
        _ => None,
    }
}

/// `input` is refused with `reason` and changes nothing.
fn assert_refused_as_is(authority: &mut Authority, input: Input, reason: Refusal) {
    let before = authority.state().clone();
    let transition = apply(authority, input);
    assert_eq!(
        refused(&transition),
        Some(reason),
        "expected the typed refusal {reason:?}, got {:?}",
        transition.effects()
    );
    assert_eq!(authority.state(), &before, "a refusal changes nothing");
}

/// The state after `inputs`, applied in order to a new set.
fn after(inputs: &[Input]) -> Authority {
    let mut authority = Authority::new();
    for input in inputs {
        apply(&mut authority, input.clone());
    }
    authority
}

/// What the machine's one answer must equal in every state.
fn none_can_still_turn_up(state: &State) -> bool {
    state.census_complete || (!state.census_owed && !state.host_inserted)
}

const ALL_INPUTS: [Input; 6] = [
    Input::ExpectCensus { owed: true },
    Input::ExpectCensus { owed: false },
    Input::BeginPass,
    Input::FinishPass,
    Input::DeclareRestored,
    Input::RegisterHostHandle,
];

#[test]
fn a_new_set_owes_no_census_and_knows_every_mob() {
    let set = Authority::new();
    let state = set.state();
    assert_eq!(state.lifecycle_phase, Phase::Unrestored);
    assert!(!state.census_owed && !state.census_complete && !state.host_inserted);
    assert!(
        state.all_mobs_known,
        "a host with no stores from before that inserts no handle has no mob that can still turn up"
    );
}

#[test]
fn whether_a_census_is_owed_is_said_before_the_first_pass() {
    // The builder says the host has stores from before: a mob no record
    // names may still turn up until the census is complete.
    let mut set = Authority::new();
    let expected = apply(&mut set, Input::ExpectCensus { owed: true });
    assert_eq!(
        effects(&expected),
        vec![Effect::CensusExpected { owed: true }]
    );
    assert!(set.state().census_owed);
    assert!(!set.state().all_mobs_known);
    // It can be said again, and unsaid, while no pass was begun.
    apply(&mut set, Input::ExpectCensus { owed: true });
    assert!(set.state().census_owed && !set.state().all_mobs_known);
    let unsaid = apply(&mut set, Input::ExpectCensus { owed: false });
    assert_eq!(
        effects(&unsaid),
        vec![Effect::CensusExpected { owed: false }]
    );
    assert!(!set.state().census_owed);
    assert!(set.state().all_mobs_known);

    // Unsaid for a host that inserts its own handles: such a host may
    // still insert a mob no record names.
    let mut inserting = after(&[
        Input::ExpectCensus { owed: true },
        Input::RegisterHostHandle,
    ]);
    apply(&mut inserting, Input::ExpectCensus { owed: false });
    assert!(!inserting.state().census_owed);
    assert!(!inserting.state().all_mobs_known);

    // Said after the host declared its restore done: the census is
    // complete and stays so.
    let mut declared = after(&[Input::DeclareRestored]);
    apply(&mut declared, Input::ExpectCensus { owed: true });
    assert!(declared.state().census_owed);
    assert!(declared.state().census_complete && declared.state().all_mobs_known);

    // Once a pass was begun the word is refused.
    for owed in [true, false] {
        let mut attempted = after(&[Input::ExpectCensus { owed: true }, Input::BeginPass]);
        assert_refused_as_is(
            &mut attempted,
            Input::ExpectCensus { owed },
            Refusal::PassAlreadyBegun,
        );
        let mut restored = after(&[
            Input::ExpectCensus { owed: true },
            Input::BeginPass,
            Input::FinishPass,
        ]);
        assert_refused_as_is(
            &mut restored,
            Input::ExpectCensus { owed },
            Refusal::PassAlreadyBegun,
        );
    }
}

#[test]
fn a_pass_is_begun_over_stores_from_before_and_finishes_once() {
    // No stores from before: there is nothing to pass over.
    let mut none_owed = Authority::new();
    assert_refused_as_is(&mut none_owed, Input::BeginPass, Refusal::NoCensusOwed);
    assert_refused_as_is(&mut none_owed, Input::FinishPass, Refusal::NoPassBegun);

    // The first pass is named, once.
    let mut set = after(&[Input::ExpectCensus { owed: true }]);
    let first = apply(&mut set, Input::BeginPass);
    assert_eq!(effects(&first), vec![Effect::PassBegun { first: true }]);
    assert_eq!(set.state().lifecycle_phase, Phase::Attempted);
    assert!(!set.state().census_complete && !set.state().all_mobs_known);
    // A pass that did not finish is followed by another, which is not the
    // first.
    let again = apply(&mut set, Input::BeginPass);
    assert_eq!(effects(&again), vec![Effect::PassBegun { first: false }]);
    assert_eq!(set.state().lifecycle_phase, Phase::Attempted);

    // A pass that ran to its end completes the census.
    let finished = apply(&mut set, Input::FinishPass);
    assert_eq!(
        effects(&finished),
        vec![Effect::PassFinished, Effect::CensusCompleted]
    );
    assert_eq!(set.state().lifecycle_phase, Phase::Restored);
    assert!(set.state().census_complete && set.state().all_mobs_known);
    // The set runs no second pass and takes no second finish.
    assert_refused_as_is(&mut set, Input::BeginPass, Refusal::AlreadyRestored);
    assert_refused_as_is(&mut set, Input::FinishPass, Refusal::AlreadyRestored);

    // A pass that finishes after the host declared its restore done: the
    // census was complete already, and the pass is recorded as finished.
    let mut declared = after(&[
        Input::ExpectCensus { owed: true },
        Input::BeginPass,
        Input::DeclareRestored,
    ]);
    let finished = apply(&mut declared, Input::FinishPass);
    assert_eq!(effects(&finished), vec![Effect::PassFinished]);
    assert_eq!(declared.state().lifecycle_phase, Phase::Restored);
}

#[test]
fn a_host_that_inserts_its_own_handles_completes_the_set_by_its_word() {
    // A host's own handle: from then on a mob no record names may still be
    // inserted, until the census is complete.
    for before in [
        vec![],
        vec![Input::ExpectCensus { owed: true }],
        vec![Input::ExpectCensus { owed: true }, Input::BeginPass],
    ] {
        let mut set = after(&before);
        let noted = apply(&mut set, Input::RegisterHostHandle);
        assert_eq!(effects(&noted), vec![Effect::HostHandleNoted]);
        assert!(set.state().host_inserted);
        assert!(!set.state().all_mobs_known, "after {before:?}");
        // The fact outlives the handle: nothing clears it, and a second
        // handle changes nothing.
        let before_second = set.state().clone();
        let second = apply(&mut set, Input::RegisterHostHandle);
        assert_eq!(effects(&second), vec![Effect::HostHandleNoted]);
        assert_eq!(set.state(), &before_second);

        // The host's word completes the set, in whatever phase.
        let phase = set.state().lifecycle_phase;
        let completed = apply(&mut set, Input::DeclareRestored);
        assert_eq!(effects(&completed), vec![Effect::CensusCompleted]);
        assert_eq!(set.state().lifecycle_phase, phase);
        assert!(set.state().census_complete && set.state().all_mobs_known);
        // Once, and a handle registered after it leaves the set complete.
        assert_refused_as_is(&mut set, Input::DeclareRestored, Refusal::AlreadyComplete);
        apply(&mut set, Input::RegisterHostHandle);
        assert!(set.state().all_mobs_known);
    }

    // After a finished pass the census is complete: a host's handle is
    // noted and the set stays complete; a declaration has nothing to add.
    let mut restored = after(&[
        Input::ExpectCensus { owed: true },
        Input::BeginPass,
        Input::FinishPass,
    ]);
    let noted = apply(&mut restored, Input::RegisterHostHandle);
    assert_eq!(effects(&noted), vec![Effect::HostHandleNoted]);
    assert!(restored.state().host_inserted && restored.state().all_mobs_known);
    assert_refused_as_is(
        &mut restored,
        Input::DeclareRestored,
        Refusal::AlreadyComplete,
    );
}

/// Every state the inputs can reach, each with the inputs that lead to it.
fn reachable() -> Vec<(State, Vec<Input>)> {
    let mut found = vec![(Authority::new().state().clone(), Vec::new())];
    let mut next = 0;
    while next < found.len() {
        let (_, path) = found[next].clone();
        for input in ALL_INPUTS {
            let mut longer = path.clone();
            longer.push(input);
            let state = after(&longer).state().clone();
            if !found.iter().any(|(known, _)| known == &state) {
                found.push((state, longer));
            }
        }
        next += 1;
    }
    found
}

#[test]
fn every_reachable_state_answers_every_input_and_no_latch_is_ever_cleared() {
    let states = reachable();
    assert!(
        states.len() > 8,
        "the walk reaches the machine's states: {}",
        states.len()
    );
    for (state, path) in &states {
        // The one answer equals the facts it is kept from, in every state.
        assert_eq!(
            state.all_mobs_known,
            none_can_still_turn_up(state),
            "after {path:?}"
        );
        assert!(
            state.lifecycle_phase != Phase::Restored || state.census_complete,
            "a finished pass completed the census: after {path:?}"
        );
        assert!(
            state.lifecycle_phase == Phase::Unrestored || state.census_owed,
            "a pass is begun only where a census is owed: after {path:?}"
        );
        for input in ALL_INPUTS {
            let mut set = after(path);
            let transition = apply(&mut set, input.clone());
            let now = set.state();
            // A refusal changes nothing.
            if refused(&transition).is_some() {
                assert_eq!(now, state, "{input:?} refused after {path:?}");
            }
            // The latches: no input clears them, and a finished pass is
            // never undone.
            assert!(
                !state.census_complete || now.census_complete,
                "{input:?} after {path:?} cleared the census"
            );
            assert!(
                !state.host_inserted || now.host_inserted,
                "{input:?} after {path:?} forgot the host's handle"
            );
            assert!(
                state.lifecycle_phase != Phase::Restored || now.lifecycle_phase == Phase::Restored,
                "{input:?} after {path:?} left Restored"
            );
            // Only the builder's word changes whether a census is owed, and
            // only before the first pass.
            if !matches!(input, Input::ExpectCensus { .. }) {
                assert_eq!(
                    now.census_owed, state.census_owed,
                    "{input:?} after {path:?}"
                );
            }
        }
    }
}

/// The set's facts are the machine's: its schema names each of them and
/// the inputs that write them.
#[test]
fn the_schema_names_the_facts_of_the_set_as_a_whole() {
    let schema = meerkat_machine_schema::catalog::dsl::dsl_mob_hosted_set_lifecycle_machine();
    let fields = schema
        .state
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    for field in [
        "census_owed",
        "census_complete",
        "host_inserted",
        "all_mobs_known",
    ] {
        assert!(
            fields.contains(&field),
            "the state names {field}: {fields:?}"
        );
    }
    let inputs = schema
        .inputs
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        inputs,
        vec![
            "ExpectCensus",
            "BeginPass",
            "FinishPass",
            "DeclareRestored",
            "RegisterHostHandle"
        ]
    );
    assert_eq!(schema.version, 1);
}
