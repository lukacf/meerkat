#![allow(clippy::unwrap_or_default)]

//! W3-H-1 / dogma #4: MobMachine canonical
//! `member_session_bindings` map — transition coverage.
//!
//! These tests exercise the DSL authority directly (no actor, no
//! session service) so the behavioural guarantees of the collapsed
//! `MemberSessionBindingChanged { epoch, agent_identity, old_session_id,
//! new_session_id }` effect are pinned at the machine level where it
//! is defined. The four semantic cases are discriminated by the
//! `(old_session_id, new_session_id)` Option pair:
//!   * `(None, Some(new))`        — fresh bind  (was `Set`)
//!   * `(Some(old), Some(new))`   — rotate      (was `Rotated`)
//!   * `(Some(prior), None)`      — release     (was `Released`)
//!   * `(None, None)`             — never emitted; DSL guards prevent it.
//!
//! The fixtures model three flows:
//!   * Fresh spawn + retire — one identity lifecycle, binding is set then
//!     released.
//!   * Spawn + respawn (spawn-over-existing) + retire — identity is
//!     rotated atomically to a new bridge session id; final retire
//!     emits a (Some, None) Changed for the new session id.
//!   * Guard enforcement — a caller that passes the wrong `replacing`
//!     witness fails the transition (no silent self-heal).
//!
//! Paired with the TLC invariant `bindings_require_known_identity`
//! these tests pin the contract that the realtime WS observer subscribes
//! to.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_mob::machines::mob_machine::{
    AgentIdentity, AgentRuntimeId, FenceToken, Generation, MobId, MobMachineAuthority,
    MobMachineEffect, MobMachineInput, MobMachineMutator, MobMachineSignal, SessionId,
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
    let session_id_for_route = releasing.clone().unwrap_or_else(SessionId::default);
    MobMachineInput::Retire {
        mob_id: MobId("test-mob".to_string()),
        agent_runtime_id: runtime_id(identity_name, generation),
        agent_identity: identity(identity_name),
        generation: Generation(generation),
        releasing,
        session_id: session_id_for_route,
    }
}

fn recover_roster_member_signal(identity_name: &str, generation: u64) -> MobMachineSignal {
    MobMachineSignal::RecoverRosterMember {
        agent_identity: identity(identity_name),
        agent_runtime_id: runtime_id(identity_name, generation),
        fence_token: FenceToken(generation),
        external_addressable: false,
    }
}

fn recover_member_session_binding_signal(
    identity_name: &str,
    generation: u64,
    bridge_sid: &str,
    replacing: Option<SessionId>,
) -> MobMachineSignal {
    MobMachineSignal::RecoverMemberSessionBinding {
        agent_identity: identity(identity_name),
        agent_runtime_id: runtime_id(identity_name, generation),
        bridge_session_id: session_id(bridge_sid),
        replacing,
    }
}

/// Pull the `MemberSessionBindingChanged` effect, matching on the
/// four-tuple of `(epoch, agent_identity, old, new)`.
fn find_binding_changed(
    transition: &meerkat_mob::machines::mob_machine::MobMachineTransition,
) -> Option<(u64, AgentIdentity, Option<SessionId>, Option<SessionId>)> {
    transition.effects().iter().find_map(|e| match e {
        MobMachineEffect::MemberSessionBindingChanged {
            epoch,
            agent_identity,
            old_session_id,
            new_session_id,
        } => Some((
            *epoch,
            agent_identity.clone(),
            old_session_id.clone(),
            new_session_id.clone(),
        )),
        _ => None,
    })
}

#[test]
fn recovery_member_session_binding_is_generated_authority_owned() {
    let mut authority = MobMachineAuthority::new();

    let rejected = authority.apply_signal(recover_member_session_binding_signal(
        "alpha",
        1,
        "bridge-a-gen1",
        None,
    ));
    assert!(
        rejected.is_err(),
        "recovery binding must fail until the member identity/runtime has been recovered",
    );

    authority
        .apply_signal(recover_roster_member_signal("alpha", 1))
        .expect("roster member recovery must be accepted");
    let transition = authority
        .apply_signal(recover_member_session_binding_signal(
            "alpha",
            1,
            "bridge-a-gen1",
            None,
        ))
        .expect("fresh recovered binding must be accepted");
    assert_eq!(
        find_binding_changed(&transition),
        Some((
            authority.state().topology_epoch,
            identity("alpha"),
            None,
            Some(session_id("bridge-a-gen1")),
        )),
    );

    let idempotent = authority
        .apply_signal(recover_member_session_binding_signal(
            "alpha",
            1,
            "bridge-a-gen1",
            Some(session_id("bridge-a-gen1")),
        ))
        .expect("same recovered binding must be idempotent");
    assert!(
        find_binding_changed(&idempotent).is_none(),
        "idempotent recovered binding must not emit a topology change",
    );

    let rotated = authority
        .apply_signal(recover_member_session_binding_signal(
            "alpha",
            1,
            "bridge-a-gen2",
            Some(session_id("bridge-a-gen1")),
        ))
        .expect("recovered replacement must be accepted when replacing matches machine state");
    assert_eq!(
        find_binding_changed(&rotated),
        Some((
            authority.state().topology_epoch,
            identity("alpha"),
            Some(session_id("bridge-a-gen1")),
            Some(session_id("bridge-a-gen2")),
        )),
    );

    let wrong_replacing = authority.apply_signal(recover_member_session_binding_signal(
        "alpha",
        1,
        "bridge-a-gen3",
        Some(session_id("bridge-a-gen1")),
    ));
    assert!(
        wrong_replacing.is_err(),
        "recovered replacement must be rejected when replacing does not match machine state",
    );
    assert_eq!(
        authority
            .state()
            .member_session_bindings
            .get(&identity("alpha")),
        Some(&session_id("bridge-a-gen2")),
        "rejected recovery replacement must not mutate the binding map",
    );
}

#[test]
fn fresh_spawn_emits_member_session_binding_changed_none_to_some() {
    let mut authority = MobMachineAuthority::new();
    let transition = MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("fresh spawn must be accepted");

    let bindings = &authority.state().member_session_bindings;
    assert_eq!(
        bindings.get(&identity("alpha")),
        Some(&session_id("bridge-a-gen1")),
        "fresh spawn inserts the identity's bridge session id into the binding map",
    );

    let changed = find_binding_changed(&transition);
    assert_eq!(
        changed,
        Some((
            authority.state().topology_epoch,
            identity("alpha"),
            None,
            Some(session_id("bridge-a-gen1")),
        )),
        "fresh spawn must emit MemberSessionBindingChanged (None -> Some) with the bound session id",
    );
}

#[test]
fn respawn_spawn_emits_member_session_binding_changed_some_to_some() {
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
        .state()
        .member_session_bindings
        .get(&identity("alpha"))
        .cloned();
    let transition = MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 2, "bridge-a-gen2", prior),
    )
    .expect("respawn spawn must be accepted when replacing witnesses the prior session id");

    assert_eq!(
        authority
            .state()
            .member_session_bindings
            .get(&identity("alpha")),
        Some(&session_id("bridge-a-gen2")),
        "respawn rotates the binding to the new bridge session id",
    );

    let changed = find_binding_changed(&transition);
    assert_eq!(
        changed,
        Some((
            authority.state().topology_epoch,
            identity("alpha"),
            Some(session_id("bridge-a-gen1")),
            Some(session_id("bridge-a-gen2")),
        )),
        "respawn must emit MemberSessionBindingChanged (Some -> Some) with the old and new session ids",
    );
}

#[test]
fn retire_after_spawn_emits_member_session_binding_changed_some_to_none() {
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("spawn must be accepted");

    let releasing = authority
        .state()
        .member_session_bindings
        .get(&identity("alpha"))
        .cloned();
    let transition = MobMachineMutator::apply(&mut authority, retire_input("alpha", 1, releasing))
        .expect("retire must be accepted when releasing witnesses the prior session id");

    assert!(
        !authority
            .state()
            .member_session_bindings
            .contains_key(&identity("alpha")),
        "retire clears the identity from the binding map",
    );

    let changed = find_binding_changed(&transition);
    assert_eq!(
        changed,
        Some((
            authority.state().topology_epoch,
            identity("alpha"),
            Some(session_id("bridge-a-gen1")),
            None,
        )),
        "retire emits MemberSessionBindingChanged (Some -> None) with the session id that was bound",
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
            .state()
            .member_session_bindings
            .contains_key(&identity("ghost")),
        "rejected Spawn must not mutate the binding map",
    );
}

#[test]
fn retire_with_none_releasing_when_bound_takes_preserving_branch() {
    // W3-H: when an identity has a realtime binding AND the caller
    // passes `releasing = None`, the DSL routes through
    // `RetireRunningPreservingBinding` (not the Releasing variant).
    // This is the retire-half of a respawn: the binding map entry is
    // intentionally preserved so the replacement spawn can emit the
    // atomic rotation (Some -> Some) `MemberSessionBindingChanged`.
    // No binding-change effect is emitted; the binding stays put.
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("spawn must be accepted");

    let transition = MobMachineMutator::apply(&mut authority, retire_input("alpha", 1, None))
        .expect("Retire with releasing=None + binding present must route to PreservingBinding");

    assert_eq!(
        authority
            .state()
            .member_session_bindings
            .get(&identity("alpha")),
        Some(&session_id("bridge-a-gen1")),
        "PreservingBinding retire must leave the binding map untouched",
    );
    assert!(
        find_binding_changed(&transition).is_none(),
        "PreservingBinding retire must emit no MemberSessionBindingChanged effect",
    );
}

#[test]
fn retire_with_some_releasing_but_wrong_value_is_rejected() {
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("spawn must be accepted");

    let error = MobMachineMutator::apply(
        &mut authority,
        retire_input("alpha", 1, Some(session_id("mismatch"))),
    )
    .expect_err("Retire with a caller-smuggled releasing witness must be rejected");
    assert!(
        format!("{error:?}").contains("GuardRejected"),
        "wrong releasing witness must fail a Retire guard: {error:?}",
    );
    assert_eq!(
        authority
            .state()
            .member_session_bindings
            .get(&identity("alpha")),
        Some(&session_id("bridge-a-gen1")),
        "rejected releasing witness must leave the binding map untouched",
    );
}

#[test]
fn bindings_require_known_identity_invariant_holds_through_spawn_retire_cycle() {
    // "Binding requires known identity" invariant (from the DSL):
    //   keys(member_session_bindings) ⊆ keys(identity_to_runtime).
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
        for key in state.member_session_bindings.keys() {
            assert!(
                state.identity_to_runtime.contains_key(key),
                "invariant violated: binding for {key:?} has no identity_to_runtime entry",
            );
        }
    };
    check_invariant(authority.state());

    let releasing = authority
        .state()
        .member_session_bindings
        .get(&identity("alpha"))
        .cloned();
    MobMachineMutator::apply(&mut authority, retire_input("alpha", 1, releasing))
        .expect("retire must be accepted");
    check_invariant(authority.state());
}

// ==========================================================================
// Track-B (R5): explicit identity-level wiring inputs.
//
// These tests exercise the Track-B `WireMembers` and `UnwireMembers`
// inputs plus the `topology_epoch` advancement + `WiringGraphChanged`
// effects the `RecomputeMobPeerOverlay` composition driver consumes.
// ==========================================================================

use meerkat_mob::machines::mob_machine::WiringEdge;

fn wire_input(a: &str, b: &str) -> MobMachineInput {
    MobMachineInput::WireMembers {
        edge: WiringEdge::new(identity(a), identity(b)),
    }
}

fn unwire_input(a: &str, b: &str) -> MobMachineInput {
    MobMachineInput::UnwireMembers {
        edge: WiringEdge::new(identity(a), identity(b)),
    }
}

#[test]
fn wire_members_inserts_edge_bumps_epoch_and_emits_wiring_graph_changed() {
    let mut authority = MobMachineAuthority::new();
    assert_eq!(authority.state().topology_epoch, 0);

    let transition = MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta"))
        .expect("wire_members accepted");

    assert!(
        authority
            .state()
            .wiring_edges
            .contains(&WiringEdge::new(identity("alpha"), identity("beta"))),
        "wiring_edges must contain the new edge after WireMembers",
    );
    assert_eq!(
        authority.state().topology_epoch,
        1,
        "topology_epoch must increment by exactly 1 per wire mutation",
    );

    let epoch_from_effect = transition.effects().iter().find_map(|e| match e {
        MobMachineEffect::WiringGraphChanged { epoch } => Some(*epoch),
        _ => None,
    });
    assert_eq!(
        epoch_from_effect,
        Some(1),
        "WiringGraphChanged must carry the post-transition epoch",
    );
}

#[test]
fn wire_members_idempotently_accepts_duplicate_edge_without_bumping_epoch() {
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta"))
        .expect("first wire accepted");
    assert_eq!(authority.state().topology_epoch, 1);

    let transition = MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta"))
        .expect("re-wiring a present edge is an idempotent no-op");
    assert!(
        transition.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::WiringTrustRepairRequested { edge }
                if edge == &WiringEdge::new(identity("alpha"), identity("beta"))
        )),
        "idempotent wire must emit generated repair authority for trust reconciliation",
    );
    assert!(
        !transition
            .effects()
            .iter()
            .any(|effect| matches!(effect, MobMachineEffect::WiringGraphChanged { .. })),
        "idempotent wire must not emit a topology mutation effect",
    );
    assert_eq!(
        authority.state().topology_epoch,
        1,
        "topology_epoch must not advance on an idempotent transition",
    );
}

#[test]
fn unwire_members_removes_edge_bumps_epoch_and_emits_wiring_graph_changed() {
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta"))
        .expect("initial wire accepted");
    assert_eq!(authority.state().topology_epoch, 1);

    let transition = MobMachineMutator::apply(&mut authority, unwire_input("alpha", "beta"))
        .expect("unwire_members accepted");

    assert!(
        !authority
            .state()
            .wiring_edges
            .contains(&WiringEdge::new(identity("alpha"), identity("beta"))),
        "wiring_edges must not contain the edge after UnwireMembers",
    );
    assert_eq!(authority.state().topology_epoch, 2);

    let epoch_from_effect = transition.effects().iter().find_map(|e| match e {
        MobMachineEffect::WiringGraphChanged { epoch } => Some(*epoch),
        _ => None,
    });
    assert_eq!(epoch_from_effect, Some(2));
}

#[test]
fn unwire_members_idempotently_accepts_absent_edge_without_bumping_epoch() {
    let mut authority = MobMachineAuthority::new();
    let transition = MobMachineMutator::apply(&mut authority, unwire_input("alpha", "beta"))
        .expect("unwiring an absent edge is an idempotent no-op");
    assert!(transition.effects().is_empty());
    assert_eq!(authority.state().topology_epoch, 0);
}

#[test]
fn topology_epoch_increments_monotonically_across_wire_mutations() {
    let mut authority = MobMachineAuthority::new();
    let starting_epoch = authority.state().topology_epoch;

    MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta")).expect("wire accepted");
    assert_eq!(authority.state().topology_epoch, starting_epoch + 1);

    MobMachineMutator::apply(&mut authority, unwire_input("alpha", "beta"))
        .expect("unwire accepted");
    assert_eq!(authority.state().topology_epoch, starting_epoch + 2);
}
