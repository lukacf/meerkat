//! W3-H-1 / dogma #4: MobMachine canonical
//! `member_realtime_bindings` map — transition coverage.
//!
//! These tests exercise the DSL authority directly (no actor, no
//! session service) so the behavioural guarantees of the new typed
//! effects (`MemberRealtimeBindingSet`, `MemberRealtimeBindingRotated`,
//! `MemberRealtimeBindingReleased`) are pinned at the machine level
//! where they are defined.
//!
//! The fixtures model three flows:
//!   * Fresh spawn + retire — one identity lifecycle, binding Set then
//!     Released.
//!   * Spawn + respawn (spawn-over-existing) + retire — identity is
//!     rotated atomically to a new bridge session id; final retire
//!     emits Released for the new session id.
//!   * Guard enforcement — a caller that passes the wrong `replacing`
//!     witness fails the transition (no silent self-heal).
//!
//! Paired with the TLC invariant `bindings_require_known_identity`
//! these tests pin the contract that the realtime WS observer (wired
//! in a follow-up PR) subscribes to.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_mob::machines::mob_machine::{
    AgentIdentity, AgentRuntimeId, FenceToken, Generation, MobMachineAuthority, MobMachineEffect,
    MobMachineInput, MobMachineMutator, SessionId,
};

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity(name.to_string())
}

fn runtime_id(identity_name: &str, generation: u64) -> AgentRuntimeId {
    AgentRuntimeId(format!("{identity_name}:{generation}"))
}

fn session_id(label: &str) -> SessionId {
    SessionId(label.to_string())
}

fn spawn_input(
    identity_name: &str,
    generation: u64,
    bridge_sid: &str,
    replacing: Option<SessionId>,
) -> MobMachineInput {
    MobMachineInput::Spawn {
        agent_identity: identity(identity_name),
        agent_runtime_id: runtime_id(identity_name, generation),
        fence_token: FenceToken(generation),
        generation: Generation(generation),
        external_addressable: false,
        bridge_session_id: session_id(bridge_sid),
        replacing,
    }
}

fn retire_input(
    identity_name: &str,
    generation: u64,
    releasing: Option<SessionId>,
) -> MobMachineInput {
    MobMachineInput::Retire {
        agent_runtime_id: runtime_id(identity_name, generation),
        agent_identity: identity(identity_name),
        releasing,
    }
}

#[test]
fn fresh_spawn_emits_member_realtime_binding_set() {
    let mut authority = MobMachineAuthority::new();
    let transition = MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("fresh spawn must be accepted");

    let bindings = &authority.state.member_realtime_bindings;
    assert_eq!(
        bindings.get(&identity("alpha")),
        Some(&session_id("bridge-a-gen1")),
        "fresh spawn inserts the identity's bridge session id into the binding map",
    );

    let set_effect = transition.effects.iter().find_map(|e| match e {
        MobMachineEffect::MemberRealtimeBindingSet {
            agent_identity,
            bridge_session_id,
        } => Some((agent_identity.clone(), bridge_session_id.clone())),
        _ => None,
    });
    assert_eq!(
        set_effect,
        Some((identity("alpha"), session_id("bridge-a-gen1"))),
        "fresh spawn must emit MemberRealtimeBindingSet with the bound session id",
    );
    assert!(
        transition.effects.iter().all(|e| !matches!(
            e,
            MobMachineEffect::MemberRealtimeBindingRotated { .. }
                | MobMachineEffect::MemberRealtimeBindingReleased { .. }
        )),
        "fresh spawn must not emit Rotated or Released",
    );
}

#[test]
fn respawn_spawn_emits_member_realtime_binding_rotated() {
    let mut authority = MobMachineAuthority::new();
    // Initial spawn — binds alpha to bridge-a-gen1.
    MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("initial spawn must be accepted");

    // Simulate the actor's handle_respawn retire-half firing Retire with
    // `releasing = current binding`, then the replacement Spawn must
    // be guarded-matched by SpawnRunningReplacing. Between those two
    // calls the binding map is transiently cleared by Retire, so
    // `replacing` for the second spawn is None (the map was cleared).
    //
    // For the W3-H-1 contract here we exercise the Replacing variant
    // directly: a second Spawn fired without an intermediate Retire
    // models the invariant that an identity that is STILL bound at
    // spawn time rotates its session pointer. This is the shape the
    // respawn flow will produce once the realtime observer lands (the
    // observer deduplicates the rotation window).
    let prior = authority
        .state
        .member_realtime_bindings
        .get(&identity("alpha"))
        .cloned();
    let transition = MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 2, "bridge-a-gen2", prior),
    )
    .expect("respawn spawn must be accepted when replacing witnesses the prior session id");

    assert_eq!(
        authority
            .state
            .member_realtime_bindings
            .get(&identity("alpha")),
        Some(&session_id("bridge-a-gen2")),
        "respawn rotates the binding to the new bridge session id",
    );

    let rotated = transition.effects.iter().find_map(|e| match e {
        MobMachineEffect::MemberRealtimeBindingRotated {
            agent_identity,
            old_session_id,
            new_session_id,
        } => Some((
            agent_identity.clone(),
            old_session_id.clone(),
            new_session_id.clone(),
        )),
        _ => None,
    });
    assert_eq!(
        rotated,
        Some((
            identity("alpha"),
            session_id("bridge-a-gen1"),
            session_id("bridge-a-gen2"),
        )),
        "respawn must emit Rotated with the old and new session ids",
    );
    assert!(
        transition.effects.iter().all(|e| !matches!(
            e,
            MobMachineEffect::MemberRealtimeBindingSet { .. }
                | MobMachineEffect::MemberRealtimeBindingReleased { .. }
        )),
        "respawn must not emit Set or Released",
    );
}

#[test]
fn retire_after_spawn_emits_member_realtime_binding_released() {
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("spawn must be accepted");

    let releasing = authority
        .state
        .member_realtime_bindings
        .get(&identity("alpha"))
        .cloned();
    let transition = MobMachineMutator::apply(&mut authority, retire_input("alpha", 1, releasing))
        .expect("retire must be accepted when releasing witnesses the prior session id");

    assert!(
        !authority
            .state
            .member_realtime_bindings
            .contains_key(&identity("alpha")),
        "retire clears the identity from the binding map",
    );

    let released = transition.effects.iter().find_map(|e| match e {
        MobMachineEffect::MemberRealtimeBindingReleased {
            agent_identity,
            session_id,
        } => Some((agent_identity.clone(), session_id.clone())),
        _ => None,
    });
    assert_eq!(
        released,
        Some((identity("alpha"), session_id("bridge-a-gen1"))),
        "retire emits Released with the session id that was bound",
    );
    assert!(
        transition.effects.iter().all(|e| !matches!(
            e,
            MobMachineEffect::MemberRealtimeBindingSet { .. }
                | MobMachineEffect::MemberRealtimeBindingRotated { .. }
        )),
        "retire must not emit Set or Rotated",
    );
}

#[test]
fn spawn_with_wrong_replacing_witness_is_rejected() {
    let mut authority = MobMachineAuthority::new();
    // No prior binding — passing `replacing = Some(_)` must fail both
    // guard variants: SpawnRunningFresh requires `replacing == None`,
    // SpawnRunningReplacing requires the state's binding map to contain
    // the identity. Neither guard matches, so the transition is
    // rejected.
    let result = MobMachineMutator::apply(
        &mut authority,
        spawn_input("ghost", 1, "bridge-g-gen1", Some(session_id("fabricated"))),
    );
    assert!(
        result.is_err(),
        "DSL must reject a Spawn whose `replacing` does not match the binding map state",
    );
    assert!(
        !authority
            .state
            .member_realtime_bindings
            .contains_key(&identity("ghost")),
        "rejected Spawn must not mutate the binding map",
    );
}

#[test]
fn retire_with_wrong_releasing_witness_is_rejected() {
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("spawn must be accepted");

    // Identity IS bound — the Retire caller must pass the correct
    // `releasing = Some(prior)`. Passing None must fail the releasing
    // variants' guards; the NoBinding variants require the map to not
    // contain the key. Neither guard matches, so the transition is
    // rejected and the binding stays put.
    let result = MobMachineMutator::apply(&mut authority, retire_input("alpha", 1, None));
    assert!(
        result.is_err(),
        "DSL must reject a Retire whose `releasing` disagrees with the binding map state",
    );
    assert_eq!(
        authority
            .state
            .member_realtime_bindings
            .get(&identity("alpha")),
        Some(&session_id("bridge-a-gen1")),
        "rejected Retire must not mutate the binding map",
    );
}

#[test]
fn bindings_require_known_identity_invariant_holds_through_spawn_retire_cycle() {
    // "Binding requires known identity" invariant (from the DSL):
    //   keys(member_realtime_bindings) ⊆ keys(identity_to_runtime).
    // Spawn inserts into both; Retire removes only from bindings;
    // identity_to_runtime stays populated (identity is still a known
    // identity for the mob even after retirement). So the subset
    // relation holds through the whole lifecycle.
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("spawn must be accepted");

    let check_invariant = |state: &meerkat_mob::machines::mob_machine::MobMachineState| {
        for key in state.member_realtime_bindings.keys() {
            assert!(
                state.identity_to_runtime.contains_key(key),
                "invariant violated: binding for {key:?} has no identity_to_runtime entry",
            );
        }
    };
    check_invariant(&authority.state);

    let releasing = authority
        .state
        .member_realtime_bindings
        .get(&identity("alpha"))
        .cloned();
    MobMachineMutator::apply(&mut authority, retire_input("alpha", 1, releasing))
        .expect("retire must be accepted");
    check_invariant(&authority.state);
}
