//! Leg B: MobMachine emit-on-revival plus the fail-closed re-admission rule.
//!
//! Post-discard revival re-materializes a member's live session but nothing on
//! that path re-establishes the consumer's placement tuple: revival prepares
//! LOCAL session resources (registration without a placement commit) and the
//! consumer's own re-admission arms clear the placement triple. So a revived
//! local member used to end up registered-but-unplaced with no lane left to
//! bind it - live, and refused by every placement-guarded consumer transition.
//!
//! These tests exercise the DSL authority directly (no actor, no session
//! service) and pin three facts at the machine level:
//!   * a LOCAL revival re-emits `RequestRuntimeBinding` carrying the exact
//!     membership tuple the machine already owns,
//!   * a PLACED revival does not (its member host owns the binding - the same
//!     reason `CommitSpawnMembershipRemote` emits none; that arm is pinned in
//!     `multi_host_machine.rs` where the host ladder lives),
//!   * a revival that cannot name the exact tuple is REFUSED rather than
//!     resolving the obligation and leaving the member unbindable.
//!
//! The refusal is load-bearing rather than defensive: DSL map accessors yield
//! the field default for an absent key, so an unguarded emit would route an
//! empty session id or a zero fence token into the consumer - a silently wrong
//! binding instead of a loud one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_mob::machines::mob_machine::{
    AgentIdentity, AgentRuntimeId, FenceToken, Generation,
    MemberLiveMaterializationObservationKind, MemberRevivalVerdictKind, MobMachineAuthority,
    MobMachineEffect, MobMachineInput, MobMachineMutator, MobMachineSignal, MobMachineTransition,
    SessionId, SpawnPolicyRuntimeMode,
};

const MEMBER: &str = "worker-1";
const GENERATION: u64 = 0;
const FENCE: u64 = 1;
const BRIDGE_SESSION: &str = "bridge-worker-1-gen0";

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity(name.to_string())
}

fn runtime_id(identity_name: &str, generation: u64) -> AgentRuntimeId {
    AgentRuntimeId(format!("{identity_name}:{generation}"))
}

fn session_id(label: &str) -> SessionId {
    SessionId(label.to_string())
}

fn profile_material_digest(identity_name: &str) -> String {
    format!("revival-profile-digest:{identity_name}")
}

fn authorize_spawn_profile(authority: &mut MobMachineAuthority, identity_name: &str) {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::AuthorizeSpawnProfile {
            agent_identity: identity(identity_name),
            profile_name: "worker".to_string(),
            model: "test-model".to_string(),
            profile_material_digest: profile_material_digest(identity_name),
            tool_config_digest: "test-tool-config-digest".to_string(),
            skills_digest: "test-skills-digest".to_string(),
            provider_params_digest: None,
            output_schema_digest: None,
            external_addressable: false,
            resolved_spec_digest: None,
        },
    )
    .expect("spawn profile material must be authorized before the spawn ladder");
}

/// Local (unplaced) spawn ladder: `BeginSpawnExec` -> `CommitSpawnMembership`
/// -> `CommitSpawnActivation`, the way the actor's spawn finalizers drive it.
fn apply_local_spawn_ladder(authority: &mut MobMachineAuthority, identity_name: &str) {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::BeginSpawnExec {
            agent_identity: identity(identity_name),
            agent_runtime_id: runtime_id(identity_name, GENERATION),
            fence_token: FenceToken(FENCE),
            generation: Generation(GENERATION),
            profile_material_digest: profile_material_digest(identity_name),
            external_addressable: false,
            runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
            bridge_session_id: Some(session_id(BRIDGE_SESSION)),
            replacing: None,
            placement: None,
            workgraph_required: false,
            rust_bundles_present: false,
            per_spawn_external_tools_present: false,
            mob_default_external_tools_present: false,
            default_llm_client_override_present: false,
            host_surface_mcp_allowlist_present: false,
            inherited_tool_filter_present: false,
            shell_env_present: false,
            mcp_stdio_env_present: false,
            mcp_http_headers_present: false,
            memory_required: false,
            mcp_required: false,
            resume_session_id: None,
            placed_spawn_id: None,
            placed_provision_operation_id: None,
            placed_operation_owner_session_id: None,
            effective_profile_override_present: false,
            effective_model_override_present: false,
        },
    )
    .expect("local BeginSpawnExec must open the spawn-exec window");
    MobMachineMutator::apply(
        authority,
        MobMachineInput::CommitSpawnMembership {
            agent_identity: identity(identity_name),
            agent_runtime_id: runtime_id(identity_name, GENERATION),
            fence_token: FenceToken(FENCE),
            generation: Generation(GENERATION),
            profile_material_digest: profile_material_digest(identity_name),
            external_addressable: false,
            runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
            bridge_session_id: Some(session_id(BRIDGE_SESSION)),
            replacing: None,
            member_peer_endpoint: None,
            spec_digest_echo: None,
            ack_engine_version: None,
            placed_spawn_id: None,
            provision_operation_id: None,
        },
    )
    .expect("local CommitSpawnMembership must publish membership");
    MobMachineMutator::apply(
        authority,
        MobMachineInput::CommitSpawnActivation {
            agent_identity: identity(identity_name),
        },
    )
    .expect("local CommitSpawnActivation must settle the spawn-exec phase");
}

fn authorize_revival(authority: &mut MobMachineAuthority, identity_name: &str) {
    let transition = authority
        .apply_signal(MobMachineSignal::ClassifyMemberLiveMaterialization {
            agent_identity: identity(identity_name),
            observation: MemberLiveMaterializationObservationKind::DurableSnapshotPresent,
            reason: "live materialization missing at dispatch".to_string(),
        })
        .expect("a durable snapshot must authorize exactly one revival attempt");
    assert!(
        transition.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::MemberLiveMaterializationClassified {
                verdict: MemberRevivalVerdictKind::ReviveAuthorized,
                ..
            }
        )),
        "revival authorization is the machine-owned verdict this fixture depends on",
    );
    assert!(
        authority
            .state()
            .member_revival_pending
            .contains(&identity(identity_name)),
        "authorization must record the revival obligation",
    );
}

fn binding_requests(
    transition: &MobMachineTransition,
) -> Vec<(
    AgentIdentity,
    AgentRuntimeId,
    FenceToken,
    Option<Generation>,
    SessionId,
)> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::RequestRuntimeBinding {
                agent_identity,
                agent_runtime_id,
                fence_token,
                generation,
                session_id,
            } => Some((
                agent_identity.clone(),
                agent_runtime_id.clone(),
                *fence_token,
                *generation,
                session_id.clone(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn local_member_revival_reemits_the_exact_runtime_binding_request() {
    let mut authority = MobMachineAuthority::new();
    authorize_spawn_profile(&mut authority, MEMBER);
    apply_local_spawn_ladder(&mut authority, MEMBER);
    authorize_revival(&mut authority, MEMBER);

    let resolved = authority
        .apply_signal(MobMachineSignal::ResolveMemberRevivalSucceeded {
            agent_identity: identity(MEMBER),
        })
        .expect("a rebindable local revival must resolve");

    assert_eq!(
        binding_requests(&resolved),
        vec![(
            identity(MEMBER),
            runtime_id(MEMBER, GENERATION),
            FenceToken(FENCE),
            Some(Generation(GENERATION)),
            session_id(BRIDGE_SESSION),
        )],
        "revival must re-emit exactly one binding request carrying the membership \
         tuple the machine already owns; a revived member with no placement is \
         refused by every placement-guarded consumer transition",
    );
    assert!(
        !authority
            .state()
            .member_revival_pending
            .contains(&identity(MEMBER)),
        "a resolved revival clears the machine-owned obligation",
    );
    assert!(
        !authority
            .state()
            .member_restore_failures
            .contains_key(&identity(MEMBER)),
        "a rebindable revival is not a restore failure",
    );
}

#[test]
fn revival_resolution_refuses_when_the_session_binding_is_gone() {
    let mut authority = MobMachineAuthority::new();
    authorize_spawn_profile(&mut authority, MEMBER);
    apply_local_spawn_ladder(&mut authority, MEMBER);
    authorize_revival(&mut authority, MEMBER);

    // The membership facts the emit reads can be mutated between authorization
    // and resolution (a concurrent retirement/rebind releases the binding).
    // Recovering that exact shape is the only way to reach the refusal.
    let mut unbindable = authority.state().clone();
    assert!(
        unbindable
            .member_session_bindings
            .remove(&identity(MEMBER))
            .is_some(),
        "the fixture must actually remove the session binding it is testing for",
    );
    let mut authority = MobMachineAuthority::recover_from_state(unbindable)
        .expect("a member without a session binding is a recoverable shape");

    authority
        .apply_signal(MobMachineSignal::ResolveMemberRevivalSucceeded {
            agent_identity: identity(MEMBER),
        })
        .expect_err(
            "a revival that cannot name the exact binding tuple must refuse: resolving \
             it would clear the obligation and leave the member permanently unbindable, \
             and emitting it would route a default-valued session id",
        );
    assert!(
        authority
            .state()
            .member_revival_pending
            .contains(&identity(MEMBER)),
        "a refused resolution must leave the obligation outstanding, so a later \
         classification can re-authorize the attempt",
    );
}

#[test]
fn revival_resolution_refuses_without_an_authorized_obligation() {
    let mut authority = MobMachineAuthority::new();
    authorize_spawn_profile(&mut authority, MEMBER);
    apply_local_spawn_ladder(&mut authority, MEMBER);

    authority
        .apply_signal(MobMachineSignal::ResolveMemberRevivalSucceeded {
            agent_identity: identity(MEMBER),
        })
        .expect_err(
            "resolution is only admissible against a machine-authorized revival \
             obligation; an unsolicited resolution must never mint a binding request",
        );
}
