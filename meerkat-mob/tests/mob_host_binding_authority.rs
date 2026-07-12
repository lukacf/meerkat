//! MobHostBindingAuthority kernel tests (plan §6.3, §14 R7/R8, §18 O2,
//! §19.L3; adjudication ledger A11/A12/A14).
//!
//! These tests exercise the DSL authority directly (no actor, no comms) so
//! the member host's admission and dedup guarantees are pinned at the machine
//! level:
//!   * host bind ladder — bind / replay / AlreadyBound / observation rejects,
//!     strictly-monotonic rebind, revoke clearing mob-scoped rows;
//!   * the materialize dedup trio — replay at the recorded tuple+digest,
//!     StaleFence below it, SpecDigestMismatch at the recorded tuple with a
//!     different digest — plus fresh/superseding admits and release-memory
//!     fencing;
//!   * the preflight observation matrix — each false observation produces
//!     exactly its typed cause;
//!   * release admission mirror + disposal recording;
//!   * turn-outcome journal dedup + generation-scoped pruning at release;
//!   * multi-mob isolation — two mobs in one authority (A14).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_mob::machines::mob_host_binding_authority::{
    AgentIdentity, FenceToken, FlowTurnOutcomeKind, Generation, HostAdmissionRejectKind, InputId,
    MaterializeRejectKind, MemberKey, MemberSessionDisposal, MobHostBindingAuthorityAuthority,
    MobHostBindingAuthorityEffect, MobHostBindingAuthorityInput, MobHostBindingAuthorityMutator,
    MobHostBindingAuthorityTransition, MobId, PeerId, PeerSigningKey, SessionId, TurnKey,
};

fn mob(name: &str) -> MobId {
    MobId(name.to_string())
}

fn peer(name: &str) -> PeerId {
    PeerId(name.to_string())
}

fn signing_key(fill: u8) -> PeerSigningKey {
    PeerSigningKey([fill; 32])
}

fn member(mob_name: &str, identity: &str) -> MemberKey {
    MemberKey::new(mob(mob_name), AgentIdentity(identity.to_string()))
}

fn turn_key(mob_name: &str, identity: &str, generation: u64, input_id: &str) -> TurnKey {
    turn_key_at_fence(mob_name, identity, generation, generation, input_id)
}

fn turn_key_at_fence(
    mob_name: &str,
    identity: &str,
    generation: u64,
    fence_token: u64,
    input_id: &str,
) -> TurnKey {
    TurnKey::new(
        mob(mob_name),
        AgentIdentity(identity.to_string()),
        Generation(generation),
        FenceToken(fence_token),
        InputId(input_id.to_string()),
    )
}

fn apply(
    authority: &mut MobHostBindingAuthorityAuthority,
    input: MobHostBindingAuthorityInput,
) -> MobHostBindingAuthorityTransition {
    MobHostBindingAuthorityMutator::apply(authority, input).expect("transition must fire")
}

fn reserve_pending(
    authority: &mut MobHostBindingAuthorityAuthority,
    turn_key: &TurnKey,
    window_start: u64,
) -> MobHostBindingAuthorityTransition {
    apply(
        authority,
        MobHostBindingAuthorityInput::ReserveTurnOutcomePending {
            turn_key: turn_key.clone(),
            window_start,
        },
    )
}

fn bind_input(mob_name: &str, peer_name: &str, epoch: u64) -> MobHostBindingAuthorityInput {
    bind_input_at_generation(mob_name, peer_name, epoch, 1)
}

fn bind_input_at_generation(
    mob_name: &str,
    peer_name: &str,
    epoch: u64,
    binding_generation: u64,
) -> MobHostBindingAuthorityInput {
    MobHostBindingAuthorityInput::ResolveHostBind {
        mob_id: mob(mob_name),
        supervisor_peer_id: peer(peer_name),
        supervisor_signing_key: signing_key(1),
        epoch,
        binding_generation,
        sender_matches_supervisor: true,
        address_matches: true,
        token_valid: true,
    }
}

fn bind(authority: &mut MobHostBindingAuthorityAuthority, mob_name: &str, epoch: u64) {
    let transition = apply(authority, bind_input(mob_name, "supervisor-a", epoch));
    assert!(
        matches!(
            transition.effects().first(),
            Some(MobHostBindingAuthorityEffect::HostBindAccepted { .. })
        ),
        "bind must be accepted, got {:?}",
        transition.effects()
    );
}

fn revoke_input(
    mob_name: &str,
    peer_name: &str,
    signing_key_fill: u8,
    epoch: u64,
) -> MobHostBindingAuthorityInput {
    MobHostBindingAuthorityInput::RevokeHostBinding {
        mob_id: mob(mob_name),
        sender_peer_id: peer(peer_name),
        sender_signing_key: signing_key(signing_key_fill),
        epoch,
        binding_generation: 1,
    }
}

fn materialize_admission_input(
    key: &MemberKey,
    generation: u64,
    fence: u64,
    digest: &str,
) -> MobHostBindingAuthorityInput {
    MobHostBindingAuthorityInput::ResolveMaterializeAdmission {
        member_key: key.clone(),
        generation: Generation(generation),
        fence_token: FenceToken(fence),
        spec_digest: digest.to_string(),
    }
}

fn record_materialized(
    authority: &mut MobHostBindingAuthorityAuthority,
    key: &MemberKey,
    generation: u64,
    fence: u64,
    session: &str,
    digest: &str,
) {
    apply(
        authority,
        MobHostBindingAuthorityInput::RecordMaterializedMember {
            member_key: key.clone(),
            generation: Generation(generation),
            fence_token: FenceToken(fence),
            session_id: SessionId(session.to_string()),
            spec_digest: digest.to_string(),
        },
    );
}

/// All-clear preflight observations; tests flip individual facts.
struct PreflightObservations {
    model_resolvable: bool,
    binding_resolvable: bool,
    env_keys_present: bool,
    stdio_commands_present: bool,
    engine_protocol_supported: bool,
    durable_sessions_required: bool,
    realm_backend_persistent: bool,
    memory_required: bool,
    memory_capability: bool,
}

impl Default for PreflightObservations {
    fn default() -> Self {
        Self {
            model_resolvable: true,
            binding_resolvable: true,
            env_keys_present: true,
            stdio_commands_present: true,
            engine_protocol_supported: true,
            durable_sessions_required: true,
            realm_backend_persistent: true,
            memory_required: true,
            memory_capability: true,
        }
    }
}

fn preflight_input(key: &MemberKey, obs: &PreflightObservations) -> MobHostBindingAuthorityInput {
    MobHostBindingAuthorityInput::ResolveMaterializePreflight {
        member_key: key.clone(),
        generation: Generation(1),
        fence_token: FenceToken(1),
        model_resolvable: obs.model_resolvable,
        binding_resolvable: obs.binding_resolvable,
        env_keys_present: obs.env_keys_present,
        stdio_commands_present: obs.stdio_commands_present,
        engine_protocol_supported: obs.engine_protocol_supported,
        durable_sessions_required: obs.durable_sessions_required,
        realm_backend_persistent: obs.realm_backend_persistent,
        memory_required: obs.memory_required,
        memory_capability: obs.memory_capability,
    }
}

fn bind_reject_cause(transition: &MobHostBindingAuthorityTransition) -> HostAdmissionRejectKind {
    transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            MobHostBindingAuthorityEffect::HostBindRejected { cause, .. } => Some(*cause),
            _ => None,
        })
        .expect("HostBindRejected effect expected")
}

fn command_reject_cause(transition: &MobHostBindingAuthorityTransition) -> HostAdmissionRejectKind {
    transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            MobHostBindingAuthorityEffect::HostCommandRejected { cause, .. } => Some(*cause),
            _ => None,
        })
        .expect("HostCommandRejected effect expected")
}

fn materialize_reject_cause(
    transition: &MobHostBindingAuthorityTransition,
) -> MaterializeRejectKind {
    transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            MobHostBindingAuthorityEffect::MaterializeRejected { cause, .. } => Some(*cause),
            _ => None,
        })
        .expect("MaterializeRejected effect expected")
}

fn release_reject_cause(transition: &MobHostBindingAuthorityTransition) -> HostAdmissionRejectKind {
    transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            MobHostBindingAuthorityEffect::ReleaseRejected { cause, .. } => Some(*cause),
            _ => None,
        })
        .expect("ReleaseRejected effect expected")
}

/// The key-alignment invariants the DSL declares, asserted natively so every
/// test leaves the authority in an invariant-satisfying state.
fn assert_key_alignment(authority: &MobHostBindingAuthorityAuthority) {
    let state = authority.state();
    let bound: Vec<_> = state.binding_phases.keys().collect();
    assert_eq!(state.supervisor_peer_ids.keys().collect::<Vec<_>>(), bound);
    assert_eq!(
        state.supervisor_signing_keys.keys().collect::<Vec<_>>(),
        bound
    );
    assert_eq!(state.supervisor_epochs.keys().collect::<Vec<_>>(), bound);

    let revoked: Vec<_> = state.revoked_supervisor_peer_ids.keys().collect();
    assert_eq!(
        state
            .revoked_supervisor_signing_keys
            .keys()
            .collect::<Vec<_>>(),
        revoked
    );
    assert_eq!(
        state.revoked_supervisor_epochs.keys().collect::<Vec<_>>(),
        revoked
    );
    for key in state.binding_phases.keys() {
        assert!(
            !state.revoked_supervisor_peer_ids.contains_key(key),
            "an active binding must supersede its revoke receipt"
        );
    }

    let materialized: Vec<_> = state.materialized_generations.keys().collect();
    assert_eq!(
        state.materialized_fences.keys().collect::<Vec<_>>(),
        materialized
    );
    assert_eq!(
        state.materialized_sessions.keys().collect::<Vec<_>>(),
        materialized
    );
    assert_eq!(
        state.materialized_spec_digests.keys().collect::<Vec<_>>(),
        materialized
    );

    let released: Vec<_> = state.release_generations.keys().collect();
    assert_eq!(state.release_fences.keys().collect::<Vec<_>>(), released);
    assert_eq!(state.release_disposals.keys().collect::<Vec<_>>(), released);

    assert_eq!(
        state.turn_outcome_kinds.keys().collect::<Vec<_>>(),
        state.turn_outcome_terminal_seqs.keys().collect::<Vec<_>>()
    );

    for key in state.materialized_generations.keys() {
        assert!(
            state.binding_phases.contains_key(&key.mob_id),
            "materialized row {key:?} requires a bound mob"
        );
    }
    for key in state.release_generations.keys() {
        assert!(
            state.binding_phases.contains_key(&key.mob_id),
            "release row {key:?} requires a bound mob"
        );
    }
    for key in state.turn_outcome_terminal_seqs.keys() {
        assert!(
            state.binding_phases.contains_key(&key.mob_id),
            "turn-outcome row {key:?} requires a bound mob"
        );
    }
}

// ---------------------------------------------------------------------------
// Host bind ladder
// ---------------------------------------------------------------------------

#[test]
fn host_bind_accept_records_binding_tuple() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    let transition = apply(&mut authority, bind_input("mob-1", "supervisor-a", 3));

    let effects = transition.effects();
    assert!(matches!(
        effects.first(),
        Some(MobHostBindingAuthorityEffect::HostBindAccepted { mob_id, supervisor_peer_id, epoch, .. })
            if *mob_id == mob("mob-1") && *supervisor_peer_id == peer("supervisor-a") && *epoch == 3
    ));

    let state = authority.state();
    assert_eq!(
        state.supervisor_peer_ids.get(&mob("mob-1")),
        Some(&peer("supervisor-a"))
    );
    assert_eq!(
        state.supervisor_signing_keys.get(&mob("mob-1")),
        Some(&signing_key(1))
    );
    assert_eq!(state.supervisor_epochs.get(&mob("mob-1")), Some(&3));
    assert!(state.binding_phases.contains_key(&mob("mob-1")));
    assert_key_alignment(&authority);
}

#[test]
fn host_bind_replay_converges_without_mutation() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 3);

    let replay = apply(&mut authority, bind_input("mob-1", "supervisor-a", 3));
    assert!(matches!(
        replay.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostBindAccepted { epoch: 3, .. })
    ));
    assert_eq!(
        authority.state().supervisor_epochs.get(&mob("mob-1")),
        Some(&3)
    );
}

#[test]
fn host_bind_second_supervisor_is_typed_already_bound_until_revoke() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 3);

    let second = apply(&mut authority, bind_input("mob-1", "supervisor-b", 9));
    assert_eq!(
        bind_reject_cause(&second),
        HostAdmissionRejectKind::AlreadyBound
    );
    assert_eq!(
        authority.state().supervisor_peer_ids.get(&mob("mob-1")),
        Some(&peer("supervisor-a")),
        "a rejected second supervisor must not disturb the recorded binding"
    );

    // A bind from the recorded supervisor at a different epoch must go
    // through rebind, not bind.
    let epoch_bump = apply(&mut authority, bind_input("mob-1", "supervisor-a", 4));
    assert_eq!(
        bind_reject_cause(&epoch_bump),
        HostAdmissionRejectKind::AlreadyBound
    );

    apply(&mut authority, revoke_input("mob-1", "supervisor-a", 1, 3));
    let rebound = apply(
        &mut authority,
        bind_input_at_generation("mob-1", "supervisor-b", 9, 2),
    );
    assert!(matches!(
        rebound.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostBindAccepted { .. })
    ));
}

#[test]
fn host_bind_observation_rejects_produce_exact_causes() {
    let cases = [
        (false, true, true, HostAdmissionRejectKind::SenderMismatch),
        (true, false, true, HostAdmissionRejectKind::AddressMismatch),
        (
            true,
            true,
            false,
            HostAdmissionRejectKind::InvalidBootstrapToken,
        ),
        (false, false, false, HostAdmissionRejectKind::SenderMismatch),
    ];
    for (sender, address, token, expected) in cases {
        let mut authority = MobHostBindingAuthorityAuthority::new();
        let transition = apply(
            &mut authority,
            MobHostBindingAuthorityInput::ResolveHostBind {
                mob_id: mob("mob-1"),
                supervisor_peer_id: peer("supervisor-a"),
                supervisor_signing_key: signing_key(1),
                epoch: 1,
                binding_generation: 1,
                sender_matches_supervisor: sender,
                address_matches: address,
                token_valid: token,
            },
        );
        assert_eq!(
            bind_reject_cause(&transition),
            expected,
            "observations (sender={sender}, address={address}, token={token})"
        );
        assert!(
            !authority.state().binding_phases.contains_key(&mob("mob-1")),
            "a rejected bind must not record a binding"
        );
    }
}

// ---------------------------------------------------------------------------
// Rebind — strictly monotonic epoch
// ---------------------------------------------------------------------------

#[test]
fn host_rebind_requires_strictly_higher_epoch() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 3);

    let rebind = |epoch: u64, peer_name: &str, sender: bool| {
        MobHostBindingAuthorityInput::ResolveHostRebind {
            mob_id: mob("mob-1"),
            supervisor_peer_id: peer(peer_name),
            supervisor_signing_key: signing_key(2),
            epoch,
            binding_generation: 1,
            sender_matches_supervisor: sender,
        }
    };

    // The recorded epoch from the RECORDED supervisor is the idempotent
    // replay ack (FLAG-2), covered by
    // `host_rebind_replay_at_recorded_epoch_idempotently_acks`; a lower
    // epoch — or the recorded epoch claimed by a different peer — stays a
    // typed StaleSupervisor.
    let lower_epoch = apply(&mut authority, rebind(2, "supervisor-a", true));
    assert_eq!(
        bind_reject_cause(&lower_epoch),
        HostAdmissionRejectKind::StaleSupervisor
    );
    let same_epoch_different_peer = apply(&mut authority, rebind(3, "supervisor-b", true));
    assert_eq!(
        bind_reject_cause(&same_epoch_different_peer),
        HostAdmissionRejectKind::StaleSupervisor
    );
    assert_eq!(
        authority.state().supervisor_epochs.get(&mob("mob-1")),
        Some(&3)
    );
    assert_eq!(
        authority.state().supervisor_peer_ids.get(&mob("mob-1")),
        Some(&peer("supervisor-a")),
        "a rejected rebind must not disturb the recorded binding"
    );

    let sender_mismatch = apply(&mut authority, rebind(4, "supervisor-b", false));
    assert_eq!(
        bind_reject_cause(&sender_mismatch),
        HostAdmissionRejectKind::SenderMismatch
    );

    // Supervisor rotation rebinds with a new peer identity at a higher epoch.
    let accepted = apply(&mut authority, rebind(4, "supervisor-b", true));
    assert!(matches!(
        accepted.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostRebindAccepted { epoch: 4, .. })
    ));
    let state = authority.state();
    assert_eq!(state.supervisor_epochs.get(&mob("mob-1")), Some(&4));
    assert_eq!(
        state.supervisor_peer_ids.get(&mob("mob-1")),
        Some(&peer("supervisor-b"))
    );
    assert_eq!(
        state.supervisor_signing_keys.get(&mob("mob-1")),
        Some(&signing_key(2))
    );
    assert_key_alignment(&authority);
}

/// FLAG-2(ii): a redelivered rebind at the recorded epoch from the recorded
/// supervisor idempotently re-acknowledges (`HostRebindAccepted` re-emitted,
/// zero mutation), so the pending-rotation retry probe can distinguish
/// "host already accepted this epoch" from actually-stale and converge.
#[test]
fn host_rebind_replay_at_recorded_epoch_idempotently_acks() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 3);

    // Rotate to supervisor-b at epoch 4.
    let rotated = apply(
        &mut authority,
        MobHostBindingAuthorityInput::ResolveHostRebind {
            mob_id: mob("mob-1"),
            supervisor_peer_id: peer("supervisor-b"),
            supervisor_signing_key: signing_key(2),
            epoch: 4,
            binding_generation: 1,
            sender_matches_supervisor: true,
        },
    );
    assert!(matches!(
        rotated.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostRebindAccepted { epoch: 4, .. })
    ));
    let state_before = authority.state().clone();

    // Redelivery of the SAME rotation converges: re-ack, no mutation.
    let replay = apply(
        &mut authority,
        MobHostBindingAuthorityInput::ResolveHostRebind {
            mob_id: mob("mob-1"),
            supervisor_peer_id: peer("supervisor-b"),
            supervisor_signing_key: signing_key(2),
            epoch: 4,
            binding_generation: 1,
            sender_matches_supervisor: true,
        },
    );
    assert!(
        matches!(
            replay.effects().first(),
            Some(MobHostBindingAuthorityEffect::HostRebindAccepted {
                mob_id,
                supervisor_peer_id,
                epoch: 4,
                ..
            }) if *mob_id == mob("mob-1") && *supervisor_peer_id == peer("supervisor-b")
        ),
        "rebind replay must re-emit HostRebindAccepted, got {:?}",
        replay.effects()
    );
    assert_eq!(
        authority.state(),
        &state_before,
        "rebind replay must not mutate the recorded binding"
    );
    assert_key_alignment(&authority);

    // The recorded epoch from a DIFFERENT peer is not a replay: typed
    // StaleSupervisor, no mutation.
    let imposter = apply(
        &mut authority,
        MobHostBindingAuthorityInput::ResolveHostRebind {
            mob_id: mob("mob-1"),
            supervisor_peer_id: peer("supervisor-c"),
            supervisor_signing_key: signing_key(3),
            epoch: 4,
            binding_generation: 1,
            sender_matches_supervisor: true,
        },
    );
    assert_eq!(
        bind_reject_cause(&imposter),
        HostAdmissionRejectKind::StaleSupervisor
    );
    assert_eq!(authority.state(), &state_before);
}

#[test]
fn host_rebind_unbound_mob_is_not_bound() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    let transition = apply(
        &mut authority,
        MobHostBindingAuthorityInput::ResolveHostRebind {
            mob_id: mob("mob-1"),
            supervisor_peer_id: peer("supervisor-a"),
            supervisor_signing_key: signing_key(1),
            epoch: 1,
            binding_generation: 1,
            sender_matches_supervisor: true,
        },
    );
    assert_eq!(
        bind_reject_cause(&transition),
        HostAdmissionRejectKind::NotBound
    );
}

// ---------------------------------------------------------------------------
// Revoke + multi-mob isolation (A14)
// ---------------------------------------------------------------------------

#[test]
fn revoke_clears_only_that_mobs_rows() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-a", 1);
    bind(&mut authority, "mob-b", 1);

    let alpha_a = member("mob-a", "alpha");
    let alpha_b = member("mob-b", "alpha");
    record_materialized(&mut authority, &alpha_a, 1, 1, "session-a", "digest-a");
    record_materialized(&mut authority, &alpha_b, 1, 1, "session-b", "digest-b");

    // Seed release memory in mob-a so revoke provably clears it too.
    let beta_a = member("mob-a", "beta");
    record_materialized(&mut authority, &beta_a, 1, 1, "session-beta", "digest-beta");
    apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordMemberRelease {
            member_key: beta_a.clone(),
            generation: Generation(1),
            fence_token: FenceToken(1),
            disposal: MemberSessionDisposal::Archived,
        },
    );

    // Seed turn-journal rows in both mobs so revoke provably clears only
    // mob-a's (DEC-5: TurnKey is mob-keyed).
    for (mob_name, input_id) in [("mob-a", "input-a"), ("mob-b", "input-b")] {
        let key = turn_key(mob_name, "alpha", 1, input_id);
        reserve_pending(&mut authority, &key, 1);
        apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: key,
                terminal_seq: 11,
                outcome: FlowTurnOutcomeKind::Completed,
            },
        );
    }
    let pending_a = turn_key("mob-a", "alpha", 1, "pending-a");
    let pending_b = turn_key("mob-b", "alpha", 1, "pending-b");
    reserve_pending(&mut authority, &pending_a, 12);
    reserve_pending(&mut authority, &pending_b, 13);

    let revoked = apply(&mut authority, revoke_input("mob-a", "supervisor-a", 1, 1));
    assert!(matches!(
        revoked.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostBindingRevoked { mob_id, supervisor_peer_id, epoch, .. })
            if *mob_id == mob("mob-a") && *supervisor_peer_id == peer("supervisor-a") && *epoch == 1
    ));

    let state = authority.state();
    assert!(!state.binding_phases.contains_key(&mob("mob-a")));
    assert!(!state.materialized_generations.contains_key(&alpha_a));
    assert!(!state.release_generations.contains_key(&beta_a));
    assert!(
        !state
            .turn_outcome_terminal_seqs
            .contains_key(&turn_key("mob-a", "alpha", 1, "input-a"))
    );
    assert!(
        !state
            .turn_outcome_pending_window_starts
            .contains_key(&pending_a)
    );
    // mob-b's binding and rows are untouched (one daemon, many mobs).
    assert!(state.binding_phases.contains_key(&mob("mob-b")));
    assert_eq!(
        state.materialized_sessions.get(&alpha_b),
        Some(&SessionId("session-b".to_string()))
    );
    assert!(
        state
            .turn_outcome_terminal_seqs
            .contains_key(&turn_key("mob-b", "alpha", 1, "input-b"))
    );
    assert!(
        state
            .turn_outcome_pending_window_starts
            .contains_key(&pending_b)
    );
    assert_key_alignment(&authority);

    let double_revoke = apply(&mut authority, revoke_input("mob-a", "supervisor-a", 1, 1));
    assert!(matches!(
        double_revoke.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostBindingRevokeReplayed { mob_id, .. })
            if *mob_id == mob("mob-a")
    ));
}

#[test]
fn revoke_requires_current_authority_and_delayed_old_retry_cannot_cross_rebind() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-r", 7);

    let attacker = apply(&mut authority, revoke_input("mob-r", "attacker", 9, 7));
    assert_eq!(
        bind_reject_cause(&attacker),
        HostAdmissionRejectKind::SenderMismatch
    );
    assert!(authority.state().binding_phases.contains_key(&mob("mob-r")));

    let wrong_epoch = apply(&mut authority, revoke_input("mob-r", "supervisor-a", 1, 8));
    assert_eq!(
        bind_reject_cause(&wrong_epoch),
        HostAdmissionRejectKind::StaleSupervisor
    );
    assert!(authority.state().binding_phases.contains_key(&mob("mob-r")));

    apply(&mut authority, revoke_input("mob-r", "supervisor-a", 1, 7));
    assert!(
        !authority.state().binding_phases.contains_key(&mob("mob-r")),
        "accepted revoke clears the active binding"
    );

    // A replacement controller uses the fresh BindHost ceremony. That
    // transition clears the old retry receipt at the same semantic boundary.
    let replacement = apply(
        &mut authority,
        MobHostBindingAuthorityInput::ResolveHostBind {
            mob_id: mob("mob-r"),
            supervisor_peer_id: peer("supervisor-b"),
            supervisor_signing_key: signing_key(2),
            epoch: 9,
            binding_generation: 2,
            sender_matches_supervisor: true,
            address_matches: true,
            token_valid: true,
        },
    );
    assert!(matches!(
        replacement.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostBindAccepted { .. })
    ));
    assert!(
        !authority
            .state()
            .revoked_supervisor_peer_ids
            .contains_key(&mob("mob-r"))
    );

    // Delayed exact retry from the old controller is evaluated against the
    // NEW active tuple and cannot revoke it even if transport trust lingers.
    let delayed_old = apply(&mut authority, revoke_input("mob-r", "supervisor-a", 1, 7));
    assert_eq!(
        bind_reject_cause(&delayed_old),
        HostAdmissionRejectKind::SenderMismatch
    );
    assert_eq!(
        authority.state().supervisor_peer_ids.get(&mob("mob-r")),
        Some(&peer("supervisor-b"))
    );
    assert_eq!(
        authority.state().supervisor_epochs.get(&mob("mob-r")),
        Some(&9)
    );
    assert_key_alignment(&authority);
}

#[test]
fn delayed_old_release_generation_is_rejected_before_touching_revived_tuple() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    let key = member("mob-r", "alpha");
    bind(&mut authority, "mob-r", 7);
    record_materialized(&mut authority, &key, 1, 1, "old-session", "digest");
    apply(&mut authority, revoke_input("mob-r", "supervisor-a", 1, 7));
    apply(
        &mut authority,
        bind_input_at_generation("mob-r", "supervisor-a", 7, 2),
    );
    record_materialized(&mut authority, &key, 1, 1, "revived-session", "digest");

    let delayed_old_command = apply(
        &mut authority,
        MobHostBindingAuthorityInput::ResolveHostCommandAdmission {
            mob_id: mob("mob-r"),
            sender_peer_id: peer("supervisor-a"),
            epoch: 7,
            binding_generation: 1,
            turn_directive_present: false,
            turn_directive_supported: true,
        },
    );
    assert_eq!(
        command_reject_cause(&delayed_old_command),
        HostAdmissionRejectKind::StaleFence,
    );
    assert_eq!(
        authority.state().materialized_sessions.get(&key),
        Some(&SessionId("revived-session".to_string())),
        "rung-0 rejection happens before release admission can touch the revived row",
    );
}

// ---------------------------------------------------------------------------
// Generic host-addressed command admission
// ---------------------------------------------------------------------------

#[test]
fn host_command_admission_matrix() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 3);

    let admission = |mob_name: &str, sender: &str, epoch: u64, present: bool, supported: bool| {
        MobHostBindingAuthorityInput::ResolveHostCommandAdmission {
            mob_id: mob(mob_name),
            sender_peer_id: peer(sender),
            epoch,
            binding_generation: 1,
            turn_directive_present: present,
            turn_directive_supported: supported,
        }
    };

    let admitted = apply(
        &mut authority,
        admission("mob-1", "supervisor-a", 3, false, false),
    );
    assert!(matches!(
        admitted.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostCommandAdmitted { epoch: 3, .. })
    ));

    // A higher epoch than recorded still admits (>=, the member-side
    // supervisor-admission posture).
    let ahead = apply(
        &mut authority,
        admission("mob-1", "supervisor-a", 4, false, false),
    );
    assert!(matches!(
        ahead.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostCommandAdmitted { .. })
    ));

    let not_bound = apply(
        &mut authority,
        admission("mob-2", "supervisor-a", 3, false, false),
    );
    assert_eq!(
        command_reject_cause(&not_bound),
        HostAdmissionRejectKind::NotBound
    );

    let wrong_sender = apply(
        &mut authority,
        admission("mob-1", "intruder", 3, false, false),
    );
    assert_eq!(
        command_reject_cause(&wrong_sender),
        HostAdmissionRejectKind::SenderMismatch
    );

    let stale = apply(
        &mut authority,
        admission("mob-1", "supervisor-a", 2, false, false),
    );
    assert_eq!(
        command_reject_cause(&stale),
        HostAdmissionRejectKind::StaleSupervisor
    );

    let unsupported_directive = apply(
        &mut authority,
        admission("mob-1", "supervisor-a", 3, true, false),
    );
    assert_eq!(
        command_reject_cause(&unsupported_directive),
        HostAdmissionRejectKind::TurnDirectiveUnsupported
    );

    let supported_directive = apply(
        &mut authority,
        admission("mob-1", "supervisor-a", 3, true, true),
    );
    assert!(matches!(
        supported_directive.effects().first(),
        Some(MobHostBindingAuthorityEffect::HostCommandAdmitted { .. })
    ));
}

// ---------------------------------------------------------------------------
// Materialize dedup trio
// ---------------------------------------------------------------------------

#[test]
fn materialize_admission_fresh_then_dedup_trio() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let key = member("mob-1", "alpha");

    let fresh = apply(
        &mut authority,
        materialize_admission_input(&key, 1, 1, "digest-1"),
    );
    assert!(matches!(
        fresh.effects().first(),
        Some(MobHostBindingAuthorityEffect::MaterializeAdmitted { .. })
    ));

    record_materialized(&mut authority, &key, 1, 1, "session-1", "digest-1");

    // Replay: same tuple + same digest returns the recorded result.
    let replay = apply(
        &mut authority,
        materialize_admission_input(&key, 1, 1, "digest-1"),
    );
    assert!(matches!(
        replay.effects().first(),
        Some(MobHostBindingAuthorityEffect::MaterializeReplay { session_id, .. })
            if *session_id == SessionId("session-1".to_string())
    ));

    // Same tuple + different digest: one idempotency key never names two
    // builds.
    let mismatch = apply(
        &mut authority,
        materialize_admission_input(&key, 1, 1, "digest-2"),
    );
    assert_eq!(
        materialize_reject_cause(&mismatch),
        MaterializeRejectKind::SpecDigestMismatch
    );

    // Lower tuple: stale fence, on both the fence and generation components.
    record_materialized(&mut authority, &key, 2, 5, "session-2", "digest-2");
    let stale_fence = apply(
        &mut authority,
        materialize_admission_input(&key, 2, 4, "digest-2"),
    );
    assert_eq!(
        materialize_reject_cause(&stale_fence),
        MaterializeRejectKind::StaleFence
    );
    let stale_generation = apply(
        &mut authority,
        materialize_admission_input(&key, 1, 9, "digest-2"),
    );
    assert_eq!(
        materialize_reject_cause(&stale_generation),
        MaterializeRejectKind::StaleFence
    );

    // Higher tuple: superseding re-materialization (revival) admits.
    let superseding = apply(
        &mut authority,
        materialize_admission_input(&key, 3, 6, "digest-3"),
    );
    assert!(matches!(
        superseding.effects().first(),
        Some(MobHostBindingAuthorityEffect::MaterializeAdmitted { .. })
    ));

    // Unbound mob: typed NotBound.
    let unbound_key = member("mob-unbound", "alpha");
    let not_bound = apply(
        &mut authority,
        materialize_admission_input(&unbound_key, 1, 1, "digest-1"),
    );
    assert_eq!(
        materialize_reject_cause(&not_bound),
        MaterializeRejectKind::NotBound
    );
    assert_key_alignment(&authority);
}

#[test]
fn materialize_admission_is_fenced_by_release_memory() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let key = member("mob-1", "alpha");

    record_materialized(&mut authority, &key, 2, 3, "session-1", "digest-1");
    apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordMemberRelease {
            member_key: key.clone(),
            generation: Generation(2),
            fence_token: FenceToken(3),
            disposal: MemberSessionDisposal::Archived,
        },
    );

    // At or below the released tuple: StaleFence from release memory.
    let at_released = apply(
        &mut authority,
        materialize_admission_input(&key, 2, 3, "digest-1"),
    );
    assert_eq!(
        materialize_reject_cause(&at_released),
        MaterializeRejectKind::StaleFence
    );
    let below_released = apply(
        &mut authority,
        materialize_admission_input(&key, 2, 2, "digest-1"),
    );
    assert_eq!(
        materialize_reject_cause(&below_released),
        MaterializeRejectKind::StaleFence
    );

    // Above the released tuple: fresh admit (new machine-issued generation).
    let above_released = apply(
        &mut authority,
        materialize_admission_input(&key, 3, 4, "digest-2"),
    );
    assert!(matches!(
        above_released.effects().first(),
        Some(MobHostBindingAuthorityEffect::MaterializeAdmitted { .. })
    ));
}

#[test]
fn record_materialized_member_rejects_tuple_regression() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let key = member("mob-1", "alpha");
    record_materialized(&mut authority, &key, 2, 2, "session-2", "digest-2");

    let regression = MobHostBindingAuthorityMutator::apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordMaterializedMember {
            member_key: key.clone(),
            generation: Generation(1),
            fence_token: FenceToken(1),
            session_id: SessionId("session-old".to_string()),
            spec_digest: "digest-old".to_string(),
        },
    );
    assert!(
        regression.is_err(),
        "recording a lower tuple over a recorded row must not fire any transition"
    );
    assert_eq!(
        authority.state().materialized_sessions.get(&key),
        Some(&SessionId("session-2".to_string()))
    );

    // Idempotent re-record at the same tuple and a same-generation fence
    // advance both land.
    record_materialized(&mut authority, &key, 2, 2, "session-2", "digest-2");
    record_materialized(&mut authority, &key, 2, 3, "session-3", "digest-3");
    assert_eq!(
        authority.state().materialized_sessions.get(&key),
        Some(&SessionId("session-3".to_string()))
    );
}

// ---------------------------------------------------------------------------
// Preflight observation matrix
// ---------------------------------------------------------------------------

#[test]
fn materialize_preflight_each_false_observation_produces_its_cause() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let key = member("mob-1", "alpha");

    let all_clear = apply(
        &mut authority,
        preflight_input(&key, &PreflightObservations::default()),
    );
    assert!(matches!(
        all_clear.effects().first(),
        Some(MobHostBindingAuthorityEffect::MaterializeAdmitted { .. })
    ));

    let cases: Vec<(PreflightObservations, MaterializeRejectKind)> = vec![
        (
            PreflightObservations {
                model_resolvable: false,
                ..PreflightObservations::default()
            },
            MaterializeRejectKind::ModelUnresolvable,
        ),
        (
            PreflightObservations {
                binding_resolvable: false,
                ..PreflightObservations::default()
            },
            MaterializeRejectKind::AuthBindingUnresolvable,
        ),
        (
            PreflightObservations {
                env_keys_present: false,
                ..PreflightObservations::default()
            },
            MaterializeRejectKind::EnvKeysMissing,
        ),
        (
            PreflightObservations {
                stdio_commands_present: false,
                ..PreflightObservations::default()
            },
            MaterializeRejectKind::McpCommandMissing,
        ),
        (
            PreflightObservations {
                engine_protocol_supported: false,
                ..PreflightObservations::default()
            },
            MaterializeRejectKind::EngineProtocolUnsupported,
        ),
        (
            PreflightObservations {
                realm_backend_persistent: false,
                ..PreflightObservations::default()
            },
            MaterializeRejectKind::RealmBackendUnavailable,
        ),
        (
            PreflightObservations {
                memory_capability: false,
                ..PreflightObservations::default()
            },
            MaterializeRejectKind::MemoryStoreUnavailable,
        ),
    ];
    for (observations, expected) in &cases {
        let transition = apply(&mut authority, preflight_input(&key, observations));
        assert_eq!(materialize_reject_cause(&transition), *expected);
    }

    // Requirement-gated observations pass when the requirement is absent.
    let ephemeral_ok = apply(
        &mut authority,
        preflight_input(
            &key,
            &PreflightObservations {
                durable_sessions_required: false,
                realm_backend_persistent: false,
                ..PreflightObservations::default()
            },
        ),
    );
    assert!(matches!(
        ephemeral_ok.effects().first(),
        Some(MobHostBindingAuthorityEffect::MaterializeAdmitted { .. })
    ));
    let no_memory_ok = apply(
        &mut authority,
        preflight_input(
            &key,
            &PreflightObservations {
                memory_required: false,
                memory_capability: false,
                ..PreflightObservations::default()
            },
        ),
    );
    assert!(matches!(
        no_memory_ok.effects().first(),
        Some(MobHostBindingAuthorityEffect::MaterializeAdmitted { .. })
    ));

    // Unbound mob rejects before any observation is consulted.
    let unbound = apply(
        &mut authority,
        preflight_input(
            &member("mob-unbound", "alpha"),
            &PreflightObservations::default(),
        ),
    );
    assert_eq!(
        materialize_reject_cause(&unbound),
        MaterializeRejectKind::NotBound
    );
}

// ---------------------------------------------------------------------------
// Release admission mirror
// ---------------------------------------------------------------------------

#[test]
fn release_admission_matrix() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let key = member("mob-1", "alpha");
    record_materialized(&mut authority, &key, 2, 3, "session-1", "digest-1");

    let admission = |key: &MemberKey, generation: u64, fence: u64| {
        MobHostBindingAuthorityInput::ResolveReleaseAdmission {
            member_key: key.clone(),
            generation: Generation(generation),
            fence_token: FenceToken(fence),
        }
    };

    let exact = apply(&mut authority, admission(&key, 2, 3));
    assert!(matches!(
        exact.effects().first(),
        Some(MobHostBindingAuthorityEffect::ReleaseAdmitted { .. })
    ));

    let stale = apply(&mut authority, admission(&key, 2, 2));
    assert_eq!(
        release_reject_cause(&stale),
        HostAdmissionRejectKind::StaleFence
    );

    let ahead = apply(&mut authority, admission(&key, 3, 4));
    assert_eq!(
        release_reject_cause(&ahead),
        HostAdmissionRejectKind::Unsupported
    );

    let unknown = apply(&mut authority, admission(&member("mob-1", "ghost"), 1, 1));
    assert_eq!(
        release_reject_cause(&unknown),
        HostAdmissionRejectKind::Unsupported
    );

    let not_bound = apply(
        &mut authority,
        admission(&member("mob-unbound", "alpha"), 1, 1),
    );
    assert_eq!(
        release_reject_cause(&not_bound),
        HostAdmissionRejectKind::NotBound
    );

    // Record the disposal, then a redelivered release replays it.
    apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordMemberRelease {
            member_key: key.clone(),
            generation: Generation(2),
            fence_token: FenceToken(3),
            disposal: MemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions,
        },
    );
    let replay = apply(&mut authority, admission(&key, 2, 3));
    assert!(matches!(
        replay.effects().first(),
        Some(MobHostBindingAuthorityEffect::ReleaseReplay { disposal, .. })
            if *disposal == MemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions
    ));

    let stale_after_release = apply(&mut authority, admission(&key, 2, 2));
    assert_eq!(
        release_reject_cause(&stale_after_release),
        HostAdmissionRejectKind::StaleFence
    );
    let ahead_after_release = apply(&mut authority, admission(&key, 3, 4));
    assert_eq!(
        release_reject_cause(&ahead_after_release),
        HostAdmissionRejectKind::Unsupported
    );
    assert_key_alignment(&authority);
}

#[test]
fn record_member_release_requires_the_recorded_tuple() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let key = member("mob-1", "alpha");
    record_materialized(&mut authority, &key, 2, 3, "session-1", "digest-1");

    let wrong_tuple = MobHostBindingAuthorityMutator::apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordMemberRelease {
            member_key: key.clone(),
            generation: Generation(2),
            fence_token: FenceToken(2),
            disposal: MemberSessionDisposal::Archived,
        },
    );
    assert!(
        wrong_tuple.is_err(),
        "release recording off the recorded tuple must not fire any transition"
    );
    assert!(
        authority
            .state()
            .materialized_generations
            .contains_key(&key)
    );
}

// ---------------------------------------------------------------------------
// Turn-outcome journal
// ---------------------------------------------------------------------------

#[test]
fn turn_outcome_dedup_replays_the_recorded_row() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let key = turn_key("mob-1", "alpha", 1, "input-1");

    // A row without an exact current materialized owner is dropped as an
    // expected stale watcher completion, even if its mob is unbound.
    let unbound = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: turn_key("mob-unbound", "alpha", 1, "input-1"),
            terminal_seq: 41,
            outcome: FlowTurnOutcomeKind::Completed,
        },
    );
    assert!(matches!(
        unbound.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeStaleDropped { .. })
    ));
    let absent = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: key.clone(),
            terminal_seq: 41,
            outcome: FlowTurnOutcomeKind::Completed,
        },
    );
    assert!(matches!(
        absent.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeStaleDropped { .. })
    ));

    let member_key = member("mob-1", "alpha");
    record_materialized(&mut authority, &member_key, 1, 1, "session-1", "digest-1");
    reserve_pending(&mut authority, &key, 1);

    let fresh = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: key.clone(),
            terminal_seq: 41,
            outcome: FlowTurnOutcomeKind::Completed,
        },
    );
    assert!(matches!(
        fresh.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeRecorded {
            terminal_seq: 41,
            ..
        })
    ));

    // Redelivery with divergent payload converges on the recorded row.
    let replay = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: key.clone(),
            terminal_seq: 99,
            outcome: FlowTurnOutcomeKind::Failed,
        },
    );
    assert!(matches!(
        replay.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeReplayed { terminal_seq, outcome, .. })
            if *terminal_seq == 41 && *outcome == FlowTurnOutcomeKind::Completed
    ));
    assert_eq!(
        authority.state().turn_outcome_terminal_seqs.get(&key),
        Some(&41)
    );
    assert_eq!(
        authority.state().turn_outcome_kinds.get(&key),
        Some(&FlowTurnOutcomeKind::Completed)
    );
}

#[test]
fn turn_outcome_ack_prunes_exact_row_without_tombstoning_delayed_commit() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let member_key = member("mob-1", "alpha");
    record_materialized(&mut authority, &member_key, 1, 1, "session-1", "digest-1");
    let acknowledged = turn_key("mob-1", "alpha", 1, "input-ack");
    let retained = turn_key("mob-1", "alpha", 1, "input-retained");
    for (key, seq) in [(&acknowledged, 41), (&retained, 42)] {
        reserve_pending(&mut authority, key, 1);
        apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: key.clone(),
                terminal_seq: seq,
                outcome: FlowTurnOutcomeKind::Completed,
            },
        );
    }

    let pruned = apply(
        &mut authority,
        MobHostBindingAuthorityInput::AcknowledgeTurnOutcome {
            turn_key: acknowledged.clone(),
        },
    );
    assert!(matches!(
        pruned.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeAcknowledged { turn_key })
            if turn_key == &acknowledged
    ));
    assert!(
        !authority
            .state()
            .turn_outcome_terminal_seqs
            .contains_key(&acknowledged)
    );
    assert!(
        authority
            .state()
            .turn_outcome_terminal_seqs
            .contains_key(&retained)
    );

    let replay = apply(
        &mut authority,
        MobHostBindingAuthorityInput::AcknowledgeTurnOutcome {
            turn_key: acknowledged.clone(),
        },
    );
    assert!(matches!(
        replay.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeAckReplay { turn_key })
            if turn_key == &acknowledged
    ));

    // The absent acknowledgement retained no negative memory. This models a
    // poll ack that raced ahead of the delayed durable-journal commit.
    reserve_pending(&mut authority, &acknowledged, 1);
    let delayed = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: acknowledged.clone(),
            terminal_seq: 99,
            outcome: FlowTurnOutcomeKind::Failed,
        },
    );
    assert!(matches!(
        delayed.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeRecorded {
            turn_key,
            terminal_seq: 99,
            ..
        }) if turn_key == &acknowledged
    ));
}

#[test]
fn release_prunes_all_member_outcomes_and_late_watcher_stays_stale() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    apply(&mut authority, bind_input("mob-2", "supervisor-b", 1));
    let key = member("mob-1", "alpha");
    let beta = member("mob-1", "beta");
    let alpha_other_mob = member("mob-2", "alpha");
    record_materialized(&mut authority, &key, 2, 2, "session-1", "digest-1");
    record_materialized(&mut authority, &beta, 2, 2, "session-2", "digest-2");
    record_materialized(
        &mut authority,
        &alpha_other_mob,
        2,
        2,
        "session-3",
        "digest-3",
    );

    let record_turn = |authority: &mut MobHostBindingAuthorityAuthority,
                       mob_name: &str,
                       identity: &str,
                       generation: u64,
                       input_id: &str| {
        let key = turn_key(mob_name, identity, generation, input_id);
        reserve_pending(authority, &key, 1);
        apply(
            authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: key,
                terminal_seq: 7,
                outcome: FlowTurnOutcomeKind::Completed,
            },
        );
    };
    // Two current rows for the released member, plus isolated rows for
    // another identity and another mob.
    record_turn(&mut authority, "mob-1", "alpha", 2, "input-1");
    record_turn(&mut authority, "mob-1", "alpha", 2, "input-2");
    record_turn(&mut authority, "mob-1", "beta", 2, "input-1");
    record_turn(&mut authority, "mob-2", "alpha", 2, "input-1");
    let released_pending = turn_key("mob-1", "alpha", 2, "pending-release");
    let retained_pending = turn_key("mob-1", "beta", 2, "pending-retained");
    reserve_pending(&mut authority, &released_pending, 10);
    reserve_pending(&mut authority, &retained_pending, 11);

    let released = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordMemberRelease {
            member_key: key.clone(),
            generation: Generation(2),
            fence_token: FenceToken(2),
            disposal: MemberSessionDisposal::Archived,
        },
    );
    assert!(matches!(
        released.effects().first(),
        Some(MobHostBindingAuthorityEffect::MemberReleaseRecorded { disposal, .. })
            if *disposal == MemberSessionDisposal::Archived
    ));

    let state = authority.state();
    assert!(!state.materialized_generations.contains_key(&key));
    assert_eq!(state.release_generations.get(&key), Some(&Generation(2)));
    assert_eq!(state.release_fences.get(&key), Some(&FenceToken(2)));
    assert_eq!(
        state.release_disposals.get(&key),
        Some(&MemberSessionDisposal::Archived)
    );

    // With no materialized owner, every row for that exact member is gone;
    // other identity/mob namespaces remain isolated.
    assert!(
        !state
            .turn_outcome_terminal_seqs
            .contains_key(&turn_key("mob-1", "alpha", 2, "input-1"))
    );
    assert!(
        !state
            .turn_outcome_terminal_seqs
            .contains_key(&turn_key("mob-1", "alpha", 2, "input-2"))
    );
    assert!(
        state
            .turn_outcome_terminal_seqs
            .contains_key(&turn_key("mob-1", "beta", 2, "input-1"))
    );
    assert!(
        state
            .turn_outcome_terminal_seqs
            .contains_key(&turn_key("mob-2", "alpha", 2, "input-1"))
    );
    assert!(
        !state
            .turn_outcome_pending_window_starts
            .contains_key(&released_pending)
    );
    assert!(
        state
            .turn_outcome_pending_window_starts
            .contains_key(&retained_pending)
    );
    let before = authority.state().turn_outcome_terminal_seqs.clone();
    let late = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: turn_key("mob-1", "alpha", 2, "late-after-release"),
            terminal_seq: 99,
            outcome: FlowTurnOutcomeKind::Failed,
        },
    );
    assert!(matches!(
        late.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeStaleDropped { .. })
    ));
    assert_eq!(authority.state().turn_outcome_terminal_seqs, before);
    assert_key_alignment(&authority);
}

#[test]
fn superseding_generations_prune_old_rows_and_bound_repeated_storage() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let member_key = member("mob-1", "alpha");

    for generation in 1..=32 {
        record_materialized(
            &mut authority,
            &member_key,
            generation,
            generation,
            &format!("session-{generation}"),
            &format!("digest-{generation}"),
        );

        if generation > 1 {
            let old = apply(
                &mut authority,
                MobHostBindingAuthorityInput::RecordTurnOutcome {
                    turn_key: turn_key("mob-1", "alpha", generation - 1, "late-old-watcher"),
                    terminal_seq: generation,
                    outcome: FlowTurnOutcomeKind::Failed,
                },
            );
            assert!(matches!(
                old.effects().first(),
                Some(MobHostBindingAuthorityEffect::TurnOutcomeStaleDropped { .. })
            ));
        }

        let current_key = turn_key("mob-1", "alpha", generation, "current");
        reserve_pending(&mut authority, &current_key, generation);
        let current = apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: current_key.clone(),
                terminal_seq: generation,
                outcome: FlowTurnOutcomeKind::Completed,
            },
        );
        assert!(matches!(
            current.effects().first(),
            Some(MobHostBindingAuthorityEffect::TurnOutcomeRecorded { .. })
        ));
        assert_eq!(
            authority.state().turn_outcome_terminal_seqs.len(),
            1,
            "superseding generation {generation} must retire every older row"
        );
        assert!(
            authority
                .state()
                .turn_outcome_terminal_seqs
                .contains_key(&current_key)
        );
        assert_key_alignment(&authority);
    }
}

#[test]
fn same_generation_higher_fence_prunes_and_rejects_old_watcher() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let member_key = member("mob-1", "alpha");
    record_materialized(
        &mut authority,
        &member_key,
        7,
        10,
        "session-old-fence",
        "digest-old-fence",
    );
    let old_key = turn_key_at_fence("mob-1", "alpha", 7, 10, "old-current");
    reserve_pending(&mut authority, &old_key, 1);
    apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: old_key.clone(),
            terminal_seq: 41,
            outcome: FlowTurnOutcomeKind::Completed,
        },
    );
    let old_pending_key = turn_key_at_fence("mob-1", "alpha", 7, 10, "pending-old-watcher");
    reserve_pending(&mut authority, &old_pending_key, 2);

    record_materialized(
        &mut authority,
        &member_key,
        7,
        11,
        "session-new-fence",
        "digest-new-fence",
    );
    assert!(
        !authority
            .state()
            .turn_outcome_terminal_seqs
            .contains_key(&old_key),
        "same-generation fence supersession retires the old row"
    );
    assert!(
        !authority
            .state()
            .turn_outcome_pending_window_starts
            .contains_key(&old_pending_key),
        "same-generation fence supersession retires old Pending"
    );

    let late_old_fence = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: old_pending_key,
            terminal_seq: 42,
            outcome: FlowTurnOutcomeKind::Failed,
        },
    );
    assert!(matches!(
        late_old_fence.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeStaleDropped { .. })
    ));

    let current_key = turn_key_at_fence("mob-1", "alpha", 7, 11, "current-fence");
    reserve_pending(&mut authority, &current_key, 2);
    let current = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: current_key.clone(),
            terminal_seq: 43,
            outcome: FlowTurnOutcomeKind::Completed,
        },
    );
    assert!(matches!(
        current.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeRecorded { .. })
    ));
    let replay = apply(
        &mut authority,
        MobHostBindingAuthorityInput::RecordTurnOutcome {
            turn_key: current_key,
            terminal_seq: 999,
            outcome: FlowTurnOutcomeKind::Failed,
        },
    );
    assert!(matches!(
        replay.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomeReplayed {
            terminal_seq: 43,
            outcome: FlowTurnOutcomeKind::Completed,
            ..
        })
    ));
}

#[test]
fn pending_and_terminal_rows_share_256_quota_across_recovery() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    bind(&mut authority, "mob-1", 1);
    let member_key = member("mob-1", "alpha");
    record_materialized(
        &mut authority,
        &member_key,
        7,
        11,
        "session-current",
        "digest-current",
    );

    for index in 0..128u64 {
        let key = turn_key_at_fence("mob-1", "alpha", 7, 11, &format!("terminal-{index}"));
        let reserved = reserve_pending(&mut authority, &key, index + 1);
        assert!(matches!(
            reserved.effects().first(),
            Some(MobHostBindingAuthorityEffect::TurnOutcomePendingReserved { .. })
        ));
        let recorded = apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: key,
                terminal_seq: index + 1,
                outcome: FlowTurnOutcomeKind::Completed,
            },
        );
        assert!(matches!(
            recorded.effects().first(),
            Some(MobHostBindingAuthorityEffect::TurnOutcomeRecorded { .. })
        ));
    }
    for index in 0..128u64 {
        let key = turn_key_at_fence("mob-1", "alpha", 7, 11, &format!("pending-{index}"));
        let reserved = reserve_pending(&mut authority, &key, 1_000 + index);
        assert!(matches!(
            reserved.effects().first(),
            Some(MobHostBindingAuthorityEffect::TurnOutcomePendingReserved { .. })
        ));
    }
    assert_eq!(authority.state().turn_outcome_terminal_seqs.len(), 128);
    assert_eq!(
        authority.state().turn_outcome_pending_window_starts.len(),
        128
    );

    let recovered_state = authority.state().clone();
    let mut recovered = MobHostBindingAuthorityAuthority::recover_from_state(recovered_state)
        .expect("mixed Pending/terminal state recovers");
    let overflow_key = turn_key_at_fence("mob-1", "alpha", 7, 11, "overflow");
    let overflow = reserve_pending(&mut recovered, &overflow_key, 9_999);
    assert!(matches!(
        overflow.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomePendingJournalFull { turn_key })
            if turn_key == &overflow_key
    ));
    assert_eq!(
        recovered.state().turn_outcome_terminal_seqs.len()
            + recovered.state().turn_outcome_pending_window_starts.len(),
        256,
        "restart must preserve the shared quota exactly"
    );

    let replay_key = turn_key_at_fence("mob-1", "alpha", 7, 11, "pending-0");
    let replay = reserve_pending(&mut recovered, &replay_key, 123_456);
    assert!(matches!(
        replay.effects().first(),
        Some(MobHostBindingAuthorityEffect::TurnOutcomePendingReplayed {
            turn_key,
            window_start: 1_000,
        }) if turn_key == &replay_key
    ));
}

// ---------------------------------------------------------------------------
// Multi-mob isolation (A14): one authority, two mobs, disjoint fact rows
// ---------------------------------------------------------------------------

#[test]
fn two_mobs_share_one_authority_without_cross_talk() {
    let mut authority = MobHostBindingAuthorityAuthority::new();
    apply(&mut authority, bind_input("mob-a", "supervisor-a", 5));
    apply(&mut authority, bind_input("mob-b", "supervisor-b", 9));

    // Same identity string in both mobs: rows are keyed apart.
    let alpha_a = member("mob-a", "alpha");
    let alpha_b = member("mob-b", "alpha");
    record_materialized(&mut authority, &alpha_a, 1, 1, "session-a", "digest-a");
    record_materialized(&mut authority, &alpha_b, 4, 4, "session-b", "digest-b");

    // mob-a's supervisor is not mob-b's supervisor.
    let cross = apply(
        &mut authority,
        MobHostBindingAuthorityInput::ResolveHostCommandAdmission {
            mob_id: mob("mob-b"),
            sender_peer_id: peer("supervisor-a"),
            epoch: 9,
            binding_generation: 1,
            turn_directive_present: false,
            turn_directive_supported: false,
        },
    );
    assert_eq!(
        command_reject_cause(&cross),
        HostAdmissionRejectKind::SenderMismatch
    );

    // A replayed materialize in mob-a cannot see mob-b's row.
    let replay_a = apply(
        &mut authority,
        materialize_admission_input(&alpha_a, 1, 1, "digest-a"),
    );
    assert!(matches!(
        replay_a.effects().first(),
        Some(MobHostBindingAuthorityEffect::MaterializeReplay { session_id, .. })
            if *session_id == SessionId("session-a".to_string())
    ));

    let state = authority.state();
    assert_eq!(state.supervisor_epochs.get(&mob("mob-a")), Some(&5));
    assert_eq!(state.supervisor_epochs.get(&mob("mob-b")), Some(&9));
    assert_eq!(
        state.materialized_generations.get(&alpha_a),
        Some(&Generation(1))
    );
    assert_eq!(
        state.materialized_generations.get(&alpha_b),
        Some(&Generation(4))
    );
    assert_key_alignment(&authority);
}

// ---------------------------------------------------------------------------
// Production-schema parity (DEC-4-REVISED / plan §21.5): the machine is NOT
// canonical, so instead of a `canonical_machine_schemas()` entry it registers
// for schema parity via this dedicated drift test — the
// `session_persistence_version_authority_matches_codegen_output` posture,
// applied to the two macro expansions of the shared DSL body.
// ---------------------------------------------------------------------------

#[test]
fn production_schema_matches_catalog_schema() {
    let catalog = meerkat_machine_schema::catalog::dsl::dsl_mob_host_binding_authority_machine_production_schema();
    let production = meerkat_mob::machine_schema_exports::mob_host_binding_authority_schema();

    catalog
        .validate()
        .expect("catalog MobHostBindingAuthority schema must validate");
    production
        .validate()
        .expect("production MobHostBindingAuthority schema must validate");

    assert_eq!(
        catalog, production,
        "MobHostBindingAuthority production schema diverged from the catalog \
         DSL. The two are the same macro body invoked twice; a mismatch means \
         the shared `mob_host_binding_authority_dsl!` invocations or the \
         attached metadata drifted."
    );
}
