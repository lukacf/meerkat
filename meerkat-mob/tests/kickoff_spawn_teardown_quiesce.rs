//! 0.7.2 disciplined shell inputs — L5 (mob-kickoff), machine-level pins.
//!
//! Worklist rows 12/13/14 (`docs-internal/dogma-audits/
//! undisciplined-shell-inputs-072.json#confirmed`): the autonomous-kickoff
//! completion waiter and the spawn-provisioning waiter are in-process
//! producers that used to fire `KickoffResolve*` / spawn-completion machine
//! inputs with no machine-minted proof the input was still legal, and
//! teardown silently wiped the obligations they were racing against.
//!
//! These tests exercise the DSL authority directly (no actor, no session
//! service) and pin the D1/D2a contract at the machine level:
//!
//! * D1 — member retire / destroy-retire admission opens a machine-owned
//!   kickoff-waiter drain obligation (`RequestKickoffQuiesce` effect);
//!   destroy admission opens the pending-spawn drain obligation
//!   (`RequestPendingSpawnQuiesceForDestroy`); the feedback inputs
//!   (`KickoffQuiesced`, `CancelPendingSpawn`) close them; retirement
//!   archival and `Destroy` are guard-blocked until closure.
//! * D2a — late kickoff resolutions and spawn completions commit as explicit
//!   typed no-ops (including in the Destroyed phase) instead of guard
//!   rejections the shell has to launder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_mob::machines::mob_machine::{
    AgentIdentity, AgentRuntimeId, FenceToken, Generation, KickoffPhase, MobId,
    MobMachineAuthority, MobMachineEffect, MobMachineInput, MobMachineMutator, MobMachineSignal,
    SessionId, SpawnPolicyRuntimeMode,
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

fn profile_material_digest(identity_name: &str, generation: u64) -> String {
    format!("test-profile-digest:{identity_name}:{generation}")
}

fn authorize_spawn_profile(
    authority: &mut MobMachineAuthority,
    identity_name: &str,
    generation: u64,
) {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::AuthorizeSpawnProfile {
            agent_identity: identity(identity_name),
            profile_name: "test".to_string(),
            model: "test-model".to_string(),
            profile_material_digest: profile_material_digest(identity_name, generation),
            tool_config_digest: "test-tool-config-digest".to_string(),
            skills_digest: "test-skills-digest".to_string(),
            provider_params_digest: None,
            output_schema_digest: None,
            external_addressable: false,
        },
    )
    .expect("spawn profile material must be authorized before Spawn");
}

fn spawn_member(authority: &mut MobMachineAuthority, identity_name: &str, bridge_sid: &str) {
    authorize_spawn_profile(authority, identity_name, 1);
    MobMachineMutator::apply(
        authority,
        MobMachineInput::Spawn {
            agent_identity: identity(identity_name),
            agent_runtime_id: runtime_id(identity_name, 1),
            fence_token: FenceToken(1),
            generation: Generation(1),
            profile_material_digest: profile_material_digest(identity_name, 1),
            external_addressable: false,
            runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
            bridge_session_id: Some(session_id(bridge_sid)),
            replacing: None,
        },
    )
    .expect("fresh spawn must be accepted");
}

/// Drive the member's kickoff into the in-flight `Starting` state through the
/// canonical inputs (the same ones the actor's start path fires).
fn mark_kickoff_starting(authority: &mut MobMachineAuthority, identity_name: &str) {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::KickoffMarkPending {
            member_id: identity(identity_name),
        },
    )
    .expect("kickoff mark pending must be accepted for a fresh member");
    MobMachineMutator::apply(
        authority,
        MobMachineInput::KickoffMarkStarting {
            member_id: identity(identity_name),
        },
    )
    .expect("kickoff mark starting must be accepted from pending");
}

fn retire_input(identity_name: &str, bridge_sid: &str) -> MobMachineInput {
    MobMachineInput::Retire {
        mob_id: MobId("test-mob".to_string()),
        agent_runtime_id: runtime_id(identity_name, 1),
        agent_identity: identity(identity_name),
        generation: Generation(1),
        releasing: Some(session_id(bridge_sid)),
        session_id: Some(session_id(bridge_sid)),
    }
}

fn archived_signal(identity_name: &str) -> MobMachineSignal {
    MobMachineSignal::ObserveMemberRetirementArchived {
        agent_identity: identity(identity_name),
        agent_runtime_id: runtime_id(identity_name, 1),
        fence_token: FenceToken(1),
    }
}

fn kickoff_quiesced(identity_name: &str) -> MobMachineInput {
    MobMachineInput::KickoffQuiesced {
        member_id: identity(identity_name),
    }
}

fn kickoff_in_flight(authority: &MobMachineAuthority, identity_name: &str) -> bool {
    let id = identity(identity_name);
    let state = authority.state();
    state.member_kickoff_pending.contains(&id)
        || state.member_kickoff_starting.contains(&id)
        || state.member_kickoff_callback_pending.contains(&id)
}

fn has_kickoff_quiesce_request(
    transition: &meerkat_mob::machines::mob_machine::MobMachineTransition,
    identity_name: &str,
) -> bool {
    transition.effects().iter().any(|effect| {
        matches!(
            effect,
            MobMachineEffect::RequestKickoffQuiesce { member_id }
                if member_id == &identity(identity_name)
        )
    })
}

fn persist_kickoff_effects(
    transition: &meerkat_mob::machines::mob_machine::MobMachineTransition,
) -> Vec<KickoffPhase> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::PersistKickoffUpdate { phase, .. } => Some(phase.clone()),
            MobMachineEffect::PersistKickoffFailureUpdate { phase, .. } => Some(phase.clone()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// D1 — retire opens the kickoff drain obligation; archival gated on closure.
// ---------------------------------------------------------------------------

#[test]
fn retire_with_inflight_kickoff_emits_quiesce_request() {
    let mut authority = MobMachineAuthority::new();
    spawn_member(&mut authority, "alpha", "bridge-a");
    mark_kickoff_starting(&mut authority, "alpha");

    let transition = MobMachineMutator::apply(&mut authority, retire_input("alpha", "bridge-a"))
        .expect("retire of a live member must be accepted");
    assert!(
        has_kickoff_quiesce_request(&transition, "alpha"),
        "member retire must open the machine-owned kickoff-waiter drain obligation",
    );
}

#[test]
fn retire_without_kickoff_still_emits_quiesce_request() {
    // The obligation is unconditional: the shell discharges it totally
    // (abort/await if a waiter exists, then `KickoffQuiesced`), so the machine
    // does not need a kickoff-presence guard split on every retire variant.
    let mut authority = MobMachineAuthority::new();
    spawn_member(&mut authority, "alpha", "bridge-a");

    let transition = MobMachineMutator::apply(&mut authority, retire_input("alpha", "bridge-a"))
        .expect("retire of a live member must be accepted");
    assert!(
        has_kickoff_quiesce_request(&transition, "alpha"),
        "retire must request kickoff quiesce even when no kickoff is in flight",
    );
}

#[test]
fn retirement_archival_is_gated_on_kickoff_quiesce_closure() {
    let mut authority = MobMachineAuthority::new();
    spawn_member(&mut authority, "alpha", "bridge-a");
    mark_kickoff_starting(&mut authority, "alpha");
    MobMachineMutator::apply(&mut authority, retire_input("alpha", "bridge-a"))
        .expect("retire accepted");

    let blocked = authority.apply_signal(archived_signal("alpha"));
    assert!(
        blocked.is_err(),
        "retirement archival must be rejected while the kickoff waiter obligation is open",
    );

    let quiesce = MobMachineMutator::apply(&mut authority, kickoff_quiesced("alpha"))
        .expect("kickoff quiesce feedback must be accepted");
    assert!(
        !kickoff_in_flight(&authority, "alpha"),
        "quiesce must move the in-flight kickoff out of pending/starting/callback-pending",
    );
    assert!(
        authority
            .state()
            .member_kickoff_cancelled
            .contains(&identity("alpha")),
        "quiescing an in-flight kickoff is a machine-recorded cancellation",
    );
    assert_eq!(
        persist_kickoff_effects(&quiesce),
        vec![KickoffPhase::Cancelled],
        "in-flight quiesce must persist the Cancelled kickoff phase",
    );

    authority
        .apply_signal(archived_signal("alpha"))
        .expect("retirement archival must commit once the kickoff obligation closed");
}

#[test]
fn kickoff_quiesced_is_total_for_idle_and_terminal_kickoffs() {
    let mut authority = MobMachineAuthority::new();
    spawn_member(&mut authority, "alpha", "bridge-a");

    // No kickoff state at all: accepted no-op.
    let idle = MobMachineMutator::apply(&mut authority, kickoff_quiesced("alpha"))
        .expect("idle kickoff quiesce must be an accepted no-op");
    assert!(
        persist_kickoff_effects(&idle).is_empty(),
        "idle quiesce must not persist any kickoff phase",
    );

    // Terminal kickoff (Started): accepted no-op that does not regress state.
    mark_kickoff_starting(&mut authority, "alpha");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::KickoffResolveStarted {
            member_id: identity("alpha"),
        },
    )
    .expect("active kickoff resolution accepted");
    let terminal = MobMachineMutator::apply(&mut authority, kickoff_quiesced("alpha"))
        .expect("terminal kickoff quiesce must be an accepted no-op");
    assert!(persist_kickoff_effects(&terminal).is_empty());
    assert!(
        authority
            .state()
            .member_kickoff_started
            .contains(&identity("alpha")),
        "quiescing a terminal kickoff must not rewrite its terminal phase",
    );
}

// ---------------------------------------------------------------------------
// D2a — late kickoff resolutions are total typed no-ops.
// ---------------------------------------------------------------------------

#[test]
fn late_kickoff_resolutions_after_quiesce_are_typed_noops() {
    let mut authority = MobMachineAuthority::new();
    spawn_member(&mut authority, "alpha", "bridge-a");
    mark_kickoff_starting(&mut authority, "alpha");
    MobMachineMutator::apply(&mut authority, kickoff_quiesced("alpha"))
        .expect("quiesce accepted");

    // The waiter task's outcome lands after teardown quiesced the kickoff.
    let late_started = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::KickoffResolveStarted {
            member_id: identity("alpha"),
        },
    )
    .expect("late KickoffResolveStarted must be an accepted typed no-op");
    assert!(persist_kickoff_effects(&late_started).is_empty());
    assert!(
        !authority
            .state()
            .member_kickoff_started
            .contains(&identity("alpha")),
        "late resolution must not resurrect a quiesced kickoff",
    );
    assert!(
        authority
            .state()
            .member_kickoff_cancelled
            .contains(&identity("alpha")),
        "machine-recorded cancellation must survive the late arrival",
    );

    let late_callback = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::KickoffResolveCallbackPending {
            member_id: identity("alpha"),
        },
    )
    .expect("late KickoffResolveCallbackPending must be an accepted typed no-op");
    assert!(persist_kickoff_effects(&late_callback).is_empty());

    let late_failed = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::KickoffResolveFailed {
            member_id: identity("alpha"),
            error: "late waiter failure".to_string(),
        },
    )
    .expect("late KickoffResolveFailed must be an accepted typed no-op");
    assert!(persist_kickoff_effects(&late_failed).is_empty());
    assert!(
        !authority
            .state()
            .member_kickoff_error
            .contains_key(&identity("alpha")),
        "late failure must not overwrite the recorded cancellation with an error",
    );

    let late_cancel = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::KickoffCancelRequested {
            member_id: identity("alpha"),
        },
    )
    .expect("late KickoffCancelRequested must be an accepted typed no-op");
    assert!(persist_kickoff_effects(&late_cancel).is_empty());
}

#[test]
fn late_kickoff_resolutions_for_unknown_member_are_typed_noops() {
    // Worklist row 13: the resolution can race member unregistration so the
    // identity may be entirely unknown to the kickoff tables by the time the
    // waiter's outcome lands. That arrival is legitimate, not an error.
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::KickoffResolveStarted {
            member_id: identity("ghost"),
        },
    )
    .expect("unknown-member late resolution must be an accepted typed no-op");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::KickoffResolveFailed {
            member_id: identity("ghost"),
            error: "runtime completion waiter failed".to_string(),
        },
    )
    .expect("unknown-member late failure must be an accepted typed no-op");
}

#[test]
fn kickoff_inputs_are_total_in_destroyed_phase() {
    let mut authority = MobMachineAuthority::new();
    MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy)
        .expect("destroy of an empty mob must be accepted");

    for input in [
        MobMachineInput::KickoffResolveStarted {
            member_id: identity("alpha"),
        },
        MobMachineInput::KickoffResolveCallbackPending {
            member_id: identity("alpha"),
        },
        MobMachineInput::KickoffResolveFailed {
            member_id: identity("alpha"),
            error: "late".to_string(),
        },
        MobMachineInput::KickoffCancelRequested {
            member_id: identity("alpha"),
        },
        MobMachineInput::KickoffQuiesced {
            member_id: identity("alpha"),
        },
        MobMachineInput::CancelPendingSpawn {
            agent_identity: identity("alpha"),
        },
    ] {
        let transition = MobMachineMutator::apply(&mut authority, input)
            .expect("post-destroy late arrivals must be accepted typed no-ops");
        assert!(
            transition.effects().is_empty(),
            "destroyed-phase late arrivals must not emit effects",
        );
    }
}

// ---------------------------------------------------------------------------
// D1 — destroy gated on kickoff + pending-spawn drain closure.
// ---------------------------------------------------------------------------

#[test]
fn destroy_is_gated_on_kickoff_waiter_quiesce() {
    let mut authority = MobMachineAuthority::new();
    spawn_member(&mut authority, "alpha", "bridge-a");
    mark_kickoff_starting(&mut authority, "alpha");

    let blocked = MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy);
    assert!(
        blocked.is_err(),
        "destroy must be rejected while a kickoff waiter obligation is open",
    );

    MobMachineMutator::apply(&mut authority, kickoff_quiesced("alpha"))
        .expect("kickoff quiesce accepted");
    MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy)
        .expect("destroy must commit once kickoff waiters quiesced");
}

#[test]
fn destroy_is_gated_on_pending_spawn_drain() {
    let mut authority = MobMachineAuthority::new();
    authority
        .apply_signal(MobMachineSignal::StageSpawn {
            agent_identity: identity("alpha"),
            session_id: session_id("bridge-a"),
        })
        .expect("stage spawn accepted");
    assert_eq!(authority.state().pending_spawn_count, 1);

    let blocked = MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy);
    assert!(
        blocked.is_err(),
        "destroy must be rejected while a pending spawn obligation is open (the \
         former silent wipe of pending_spawn_count/pending_spawn_sessions is gone)",
    );

    let cancel = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::CancelPendingSpawn {
            agent_identity: identity("alpha"),
        },
    )
    .expect("typed pending-spawn cancellation must be accepted");
    assert!(
        cancel
            .effects()
            .iter()
            .all(|effect| !matches!(effect, MobMachineEffect::EmitMemberLifecycleNotice { .. })),
        "teardown cancellation must not launder a Spawned lifecycle notice",
    );
    assert_eq!(authority.state().pending_spawn_count, 0);
    assert!(authority.state().pending_spawn_sessions.is_empty());

    // Idempotent re-drain: absent identity is an accepted no-op.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::CancelPendingSpawn {
            agent_identity: identity("alpha"),
        },
    )
    .expect("absent pending-spawn cancellation must be an accepted typed no-op");

    MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy)
        .expect("destroy must commit once the pending-spawn table drained");
}

#[test]
fn destroy_admission_emits_pending_spawn_quiesce_request() {
    let mut authority = MobMachineAuthority::new();
    let transition = authority
        .apply_signal(MobMachineSignal::AdmitDestroyCleanup)
        .expect("destroy admission accepted");
    assert!(
        transition
            .effects()
            .iter()
            .any(|effect| matches!(effect, MobMachineEffect::RequestPendingSpawnQuiesceForDestroy)),
        "destroy admission must open the pending-spawn drain obligation",
    );
}

#[test]
fn admit_destroy_member_retire_emits_kickoff_quiesce_request() {
    let mut authority = MobMachineAuthority::new();
    spawn_member(&mut authority, "alpha", "bridge-a");
    mark_kickoff_starting(&mut authority, "alpha");
    authority
        .apply_signal(MobMachineSignal::AdmitDestroyCleanup)
        .expect("destroy admission accepted");

    let transition = authority
        .apply_signal(MobMachineSignal::AdmitDestroyMemberRetire {
            agent_identity: identity("alpha"),
            agent_runtime_id: runtime_id("alpha", 1),
            fence_token: FenceToken(1),
            session_id: Some(session_id("bridge-a")),
        })
        .expect("destroy member retire admission accepted");
    assert!(
        has_kickoff_quiesce_request(&transition, "alpha"),
        "destroy-retire admission must open the kickoff-waiter drain obligation",
    );
}

// ---------------------------------------------------------------------------
// D2a — late spawn completions are total typed no-ops.
// ---------------------------------------------------------------------------

#[test]
fn late_complete_spawn_is_typed_noop() {
    let mut authority = MobMachineAuthority::new();

    // No pending spawn for this identity: today's guard rejection becomes an
    // accepted typed no-op (legitimate post-drain straggler).
    authority
        .apply_signal(MobMachineSignal::CompleteSpawn {
            agent_identity: identity("ghost"),
        })
        .expect("late spawn completion must be an accepted typed no-op");

    MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy)
        .expect("destroy of an empty mob accepted");
    authority
        .apply_signal(MobMachineSignal::CompleteSpawn {
            agent_identity: identity("ghost"),
        })
        .expect("post-destroy spawn completion must be an accepted typed no-op");
}
