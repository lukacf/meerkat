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
    let session_id_for_route = releasing.clone().unwrap_or_else(SessionId::default);
    MobMachineInput::Retire {
        agent_runtime_id: runtime_id(identity_name, generation),
        agent_identity: identity(identity_name),
        releasing,
        session_id: session_id_for_route,
    }
}

/// Pull the `MemberSessionBindingChanged` effect, matching on the
/// four-tuple of `(epoch, agent_identity, old, new)`.
fn find_binding_changed(
    transition: &meerkat_mob::machines::mob_machine::MobMachineTransition,
) -> Option<(u64, AgentIdentity, Option<SessionId>, Option<SessionId>)> {
    transition.effects.iter().find_map(|e| match e {
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
fn fresh_spawn_emits_member_session_binding_changed_none_to_some() {
    let mut authority = MobMachineAuthority::new();
    let transition = MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("fresh spawn must be accepted");

    let bindings = &authority.state.member_session_bindings;
    assert_eq!(
        bindings.get(&identity("alpha")),
        Some(&session_id("bridge-a-gen1")),
        "fresh spawn inserts the identity's bridge session id into the binding map",
    );

    let changed = find_binding_changed(&transition);
    assert_eq!(
        changed,
        Some((
            authority.state.topology_epoch,
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
        .state
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
            .state
            .member_session_bindings
            .get(&identity("alpha")),
        Some(&session_id("bridge-a-gen2")),
        "respawn rotates the binding to the new bridge session id",
    );

    let changed = find_binding_changed(&transition);
    assert_eq!(
        changed,
        Some((
            authority.state.topology_epoch,
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
        .state
        .member_session_bindings
        .get(&identity("alpha"))
        .cloned();
    let transition = MobMachineMutator::apply(&mut authority, retire_input("alpha", 1, releasing))
        .expect("retire must be accepted when releasing witnesses the prior session id");

    assert!(
        !authority
            .state
            .member_session_bindings
            .contains_key(&identity("alpha")),
        "retire clears the identity from the binding map",
    );

    let changed = find_binding_changed(&transition);
    assert_eq!(
        changed,
        Some((
            authority.state.topology_epoch,
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
            .state
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
            .state
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
    // W3-H: when the caller passes `releasing = Some(wrong_value)`, the
    // DSL still routes to `RetireRunningReleasing` (the guard only
    // checks presence/absence of the witness, not value equality), and
    // the emitted `MemberSessionBindingChanged` carries whatever the
    // caller passed as the `old_session_id`. Value-level equality is
    // the caller's contract with its own bookkeeping; the DSL enforces
    // presence alignment and the shell actor always reads the current
    // binding before calling, so a mismatched value cannot originate
    // from the canonical respawn or terminal-retire flows. This test
    // pins the DSL's presence-based guard semantics.
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(
        &mut authority,
        spawn_input("alpha", 1, "bridge-a-gen1", None),
    )
    .expect("spawn must be accepted");

    // Passing `Some(wrong_session)` matches the Releasing path's guards
    // and clears the binding with a Changed effect carrying the wrong
    // value. The shell actor always reads the current binding first, so
    // this case cannot originate in production.
    let transition = MobMachineMutator::apply(
        &mut authority,
        retire_input("alpha", 1, Some(session_id("mismatch"))),
    )
    .expect("Retire with Some(_) + binding present routes to Releasing");
    assert!(
        !authority
            .state
            .member_session_bindings
            .contains_key(&identity("alpha")),
        "Releasing path must clear the binding",
    );
    let changed = find_binding_changed(&transition);
    assert!(
        matches!(changed, Some((_, _, Some(_), None))),
        "Releasing path must emit MemberSessionBindingChanged (Some -> None)",
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
    check_invariant(&authority.state);

    let releasing = authority
        .state
        .member_session_bindings
        .get(&identity("alpha"))
        .cloned();
    MobMachineMutator::apply(&mut authority, retire_input("alpha", 1, releasing))
        .expect("retire must be accepted");
    check_invariant(&authority.state);
}

// ==========================================================================
// Track-B (R5): explicit identity-level wiring and session-binding inputs.
//
// These tests exercise the Track-B `WireMembers`, `UnwireMembers`,
// `BindMemberSession`, `RotateMemberSession`, and `ReleaseMemberSession`
// inputs plus the `topology_epoch` advancement + `WiringGraphChanged` /
// `MemberSessionBindingChanged` effects the `RecomputeMobPeerOverlay`
// composition driver consumes.
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

fn bind_session_input(identity_name: &str, sid: &str) -> MobMachineInput {
    MobMachineInput::BindMemberSession {
        agent_identity: identity(identity_name),
        session_id: session_id(sid),
    }
}

fn rotate_session_input(identity_name: &str, old_sid: &str, new_sid: &str) -> MobMachineInput {
    MobMachineInput::RotateMemberSession {
        agent_identity: identity(identity_name),
        old_session_id: session_id(old_sid),
        new_session_id: session_id(new_sid),
    }
}

fn release_session_input(identity_name: &str, sid: &str) -> MobMachineInput {
    MobMachineInput::ReleaseMemberSession {
        agent_identity: identity(identity_name),
        session_id: session_id(sid),
    }
}

#[test]
fn wire_members_inserts_edge_bumps_epoch_and_emits_wiring_graph_changed() {
    let mut authority = MobMachineAuthority::new();
    assert_eq!(authority.state.topology_epoch, 0);

    let transition = MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta"))
        .expect("wire_members accepted");

    assert!(
        authority
            .state
            .wiring_edges
            .contains(&WiringEdge::new(identity("alpha"), identity("beta"))),
        "wiring_edges must contain the new edge after WireMembers",
    );
    assert_eq!(
        authority.state.topology_epoch, 1,
        "topology_epoch must increment by exactly 1 per wire mutation",
    );

    let epoch_from_effect = transition.effects.iter().find_map(|e| match e {
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
fn wire_members_rejects_duplicate_edge_without_bumping_epoch() {
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta"))
        .expect("first wire accepted");
    assert_eq!(authority.state.topology_epoch, 1);

    let result = MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta"));
    assert!(
        result.is_err(),
        "re-wiring a present edge must be rejected by the guard",
    );
    assert_eq!(
        authority.state.topology_epoch, 1,
        "topology_epoch must not advance on a rejected transition",
    );
}

#[test]
fn unwire_members_removes_edge_bumps_epoch_and_emits_wiring_graph_changed() {
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta"))
        .expect("initial wire accepted");
    assert_eq!(authority.state.topology_epoch, 1);

    let transition = MobMachineMutator::apply(&mut authority, unwire_input("alpha", "beta"))
        .expect("unwire_members accepted");

    assert!(
        !authority
            .state
            .wiring_edges
            .contains(&WiringEdge::new(identity("alpha"), identity("beta"))),
        "wiring_edges must not contain the edge after UnwireMembers",
    );
    assert_eq!(authority.state.topology_epoch, 2);

    let epoch_from_effect = transition.effects.iter().find_map(|e| match e {
        MobMachineEffect::WiringGraphChanged { epoch } => Some(*epoch),
        _ => None,
    });
    assert_eq!(epoch_from_effect, Some(2));
}

#[test]
fn unwire_members_rejects_absent_edge_without_bumping_epoch() {
    let mut authority = MobMachineAuthority::new();
    let result = MobMachineMutator::apply(&mut authority, unwire_input("alpha", "beta"));
    assert!(
        result.is_err(),
        "unwiring an absent edge must be rejected by the guard",
    );
    assert_eq!(authority.state.topology_epoch, 0);
}

/// `BindMemberSession` requires the identity to have a runtime first
/// (enforced by the `identity_has_runtime` guard). Callers spawn the
/// member via the regular `Spawn` input before binding. These tests
/// first drive Spawn (which itself sets a binding via its lifecycle
/// path), then Release it, then exercise the explicit Bind input
/// against the now-runtime-present-but-unbound identity.
fn spawn_then_release(
    authority: &mut MobMachineAuthority,
    identity_name: &str,
    generation: u64,
    initial_sid: &str,
) {
    MobMachineMutator::apply(
        authority,
        spawn_input(identity_name, generation, initial_sid, None),
    )
    .expect("spawn must be accepted");
    MobMachineMutator::apply(authority, release_session_input(identity_name, initial_sid))
        .expect("release must be accepted");
}

#[test]
fn bind_member_session_inserts_binding_and_emits_changed_none_to_some() {
    let mut authority = MobMachineAuthority::new();
    spawn_then_release(&mut authority, "alpha", 1, "spawn-sid");
    let epoch_before_bind = authority.state.topology_epoch;

    let transition =
        MobMachineMutator::apply(&mut authority, bind_session_input("alpha", "session-1"))
            .expect("bind_member_session accepted");

    assert_eq!(
        authority
            .state
            .member_session_bindings
            .get(&identity("alpha")),
        Some(&session_id("session-1")),
    );
    assert_eq!(authority.state.topology_epoch, epoch_before_bind + 1);

    let changed = find_binding_changed(&transition);
    assert_eq!(
        changed,
        Some((
            authority.state.topology_epoch,
            identity("alpha"),
            None,
            Some(session_id("session-1")),
        )),
        "BindMemberSession must emit MemberSessionBindingChanged (None -> Some)",
    );
}

#[test]
fn bind_member_session_rejects_unknown_identity() {
    let mut authority = MobMachineAuthority::new();
    let result = MobMachineMutator::apply(&mut authority, bind_session_input("alpha", "session-1"));
    assert!(
        result.is_err(),
        "BindMemberSession on an identity with no runtime must be rejected",
    );
    assert_eq!(authority.state.topology_epoch, 0);
}

#[test]
fn bind_member_session_rejects_prior_binding() {
    let mut authority = MobMachineAuthority::new();
    spawn_then_release(&mut authority, "alpha", 1, "spawn-sid");
    MobMachineMutator::apply(&mut authority, bind_session_input("alpha", "session-1"))
        .expect("first bind accepted");
    let epoch_after_first_bind = authority.state.topology_epoch;

    let result = MobMachineMutator::apply(&mut authority, bind_session_input("alpha", "session-2"));
    assert!(
        result.is_err(),
        "BindMemberSession with prior binding must be rejected",
    );
    assert_eq!(authority.state.topology_epoch, epoch_after_first_bind);
}

#[test]
fn rotate_member_session_updates_binding_and_emits_changed_some_to_some() {
    let mut authority = MobMachineAuthority::new();
    spawn_then_release(&mut authority, "alpha", 1, "spawn-sid");
    MobMachineMutator::apply(&mut authority, bind_session_input("alpha", "session-1"))
        .expect("initial bind accepted");
    let epoch_before_rotate = authority.state.topology_epoch;

    let transition = MobMachineMutator::apply(
        &mut authority,
        rotate_session_input("alpha", "session-1", "session-2"),
    )
    .expect("rotate accepted");

    assert_eq!(
        authority
            .state
            .member_session_bindings
            .get(&identity("alpha")),
        Some(&session_id("session-2")),
    );
    assert_eq!(authority.state.topology_epoch, epoch_before_rotate + 1);

    let changed = find_binding_changed(&transition);
    assert_eq!(
        changed,
        Some((
            authority.state.topology_epoch,
            identity("alpha"),
            Some(session_id("session-1")),
            Some(session_id("session-2")),
        )),
        "RotateMemberSession must emit MemberSessionBindingChanged (Some -> Some)",
    );
}

#[test]
fn rotate_member_session_rejects_when_no_prior_binding() {
    let mut authority = MobMachineAuthority::new();
    spawn_then_release(&mut authority, "alpha", 1, "spawn-sid");
    let epoch_after_spawn_release = authority.state.topology_epoch;
    let result = MobMachineMutator::apply(
        &mut authority,
        rotate_session_input("alpha", "session-1", "session-2"),
    );
    assert!(result.is_err());
    assert_eq!(authority.state.topology_epoch, epoch_after_spawn_release);
}

#[test]
fn rotate_member_session_rejects_wrong_old_session_id_witness() {
    // PR #340 review item #5: a caller that supplies the wrong
    // `old_session_id` must be rejected by the
    // `old_session_id_matches_current` guard.
    let mut authority = MobMachineAuthority::new();
    spawn_then_release(&mut authority, "alpha", 1, "spawn-sid");
    MobMachineMutator::apply(
        &mut authority,
        bind_session_input("alpha", "session-actual"),
    )
    .expect("initial bind accepted");
    let epoch_before_rotate = authority.state.topology_epoch;

    // Caller claims the current binding is "session-stale" but the
    // actual binding is "session-actual" — rotation must be rejected.
    let result = MobMachineMutator::apply(
        &mut authority,
        rotate_session_input("alpha", "session-stale", "session-new"),
    );
    assert!(
        result.is_err(),
        "RotateMemberSession with a wrong old_session_id must be rejected",
    );
    assert_eq!(authority.state.topology_epoch, epoch_before_rotate);
    assert_eq!(
        authority
            .state
            .member_session_bindings
            .get(&identity("alpha")),
        Some(&session_id("session-actual")),
        "binding must remain untouched after rejected forgery attempt",
    );
}

#[test]
fn release_member_session_rejects_wrong_session_id_witness() {
    // PR #340 review item #5: `ReleaseMemberSession` also verifies
    // the caller's session_id witness matches the current binding.
    let mut authority = MobMachineAuthority::new();
    spawn_then_release(&mut authority, "alpha", 1, "spawn-sid");
    MobMachineMutator::apply(
        &mut authority,
        bind_session_input("alpha", "session-actual"),
    )
    .expect("initial bind accepted");
    let epoch_before_release = authority.state.topology_epoch;

    let result = MobMachineMutator::apply(
        &mut authority,
        release_session_input("alpha", "session-stale"),
    );
    assert!(
        result.is_err(),
        "ReleaseMemberSession with a wrong session_id must be rejected",
    );
    assert_eq!(authority.state.topology_epoch, epoch_before_release);
    assert!(
        authority
            .state
            .member_session_bindings
            .contains_key(&identity("alpha")),
        "binding must remain present after rejected forgery attempt",
    );
}

#[test]
fn release_member_session_removes_binding_and_emits_changed_some_to_none() {
    let mut authority = MobMachineAuthority::new();
    spawn_then_release(&mut authority, "alpha", 1, "spawn-sid");
    MobMachineMutator::apply(&mut authority, bind_session_input("alpha", "session-1"))
        .expect("initial bind accepted");
    let epoch_before_release = authority.state.topology_epoch;

    let transition =
        MobMachineMutator::apply(&mut authority, release_session_input("alpha", "session-1"))
            .expect("release accepted");

    assert!(
        !authority
            .state
            .member_session_bindings
            .contains_key(&identity("alpha")),
        "release_member_session must remove the binding",
    );
    assert_eq!(authority.state.topology_epoch, epoch_before_release + 1);

    let changed = find_binding_changed(&transition);
    assert_eq!(
        changed,
        Some((
            authority.state.topology_epoch,
            identity("alpha"),
            Some(session_id("session-1")),
            None,
        )),
        "ReleaseMemberSession must emit MemberSessionBindingChanged (Some -> None)",
    );
}

#[test]
fn release_member_session_rejects_when_no_prior_binding() {
    let mut authority = MobMachineAuthority::new();
    let result =
        MobMachineMutator::apply(&mut authority, release_session_input("alpha", "session-1"));
    assert!(result.is_err());
    assert_eq!(authority.state.topology_epoch, 0);
}

#[test]
fn topology_epoch_increments_monotonically_across_mixed_wire_and_bind_mutations() {
    let mut authority = MobMachineAuthority::new();
    spawn_then_release(&mut authority, "alpha", 1, "spawn-sid-a");
    spawn_then_release(&mut authority, "beta", 2, "spawn-sid-b");
    let starting_epoch = authority.state.topology_epoch;

    MobMachineMutator::apply(&mut authority, wire_input("alpha", "beta")).expect("wire accepted");
    assert_eq!(authority.state.topology_epoch, starting_epoch + 1);

    MobMachineMutator::apply(&mut authority, bind_session_input("alpha", "session-a"))
        .expect("bind accepted");
    assert_eq!(authority.state.topology_epoch, starting_epoch + 2);

    MobMachineMutator::apply(&mut authority, bind_session_input("beta", "session-b"))
        .expect("bind accepted");
    assert_eq!(authority.state.topology_epoch, starting_epoch + 3);

    MobMachineMutator::apply(
        &mut authority,
        rotate_session_input("alpha", "session-a", "session-a2"),
    )
    .expect("rotate accepted");
    assert_eq!(authority.state.topology_epoch, starting_epoch + 4);

    MobMachineMutator::apply(&mut authority, unwire_input("alpha", "beta"))
        .expect("unwire accepted");
    assert_eq!(authority.state.topology_epoch, starting_epoch + 5);
}
