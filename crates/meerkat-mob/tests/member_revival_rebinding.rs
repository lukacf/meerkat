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
//! service) and pin these facts at the machine level:
//!   * a LOCAL revival re-emits `RequestRuntimeBinding` carrying the exact
//!     membership tuple the machine already owns, in its Bound step; the
//!     Ready step then closes the wake attempt Live,
//!   * a PLACED revival does not (its member host owns the binding - the same
//!     reason `CommitSpawnMembershipRemote` emits none; that arm is pinned in
//!     `multi_host_machine.rs` where the host ladder lives),
//!   * every resolution names its wake attempt: feedback for any other
//!     attempt is a typed `StaleAttempt` no-op, and an open attempt never
//!     outlives the session binding it would name.
//!
//! The tuple guards are load-bearing rather than defensive: DSL map accessors
//! yield the field default for an absent key, so an unguarded emit would route
//! an empty session id or a zero fence token into the consumer - a silently
//! wrong binding instead of a loud one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_mob::machines::mob_machine::{
    AgentIdentity, AgentRuntimeId, FenceToken, Generation,
    MemberLiveMaterializationObservationKind, MemberMaterializationPhase, MemberPeerEndpoint,
    MemberRevivalVerdictKind, MemberWakeFeedbackVerdict, MobMachineAuthority, MobMachineEffect,
    MobMachineInput, MobMachineMutator, MobMachineSignal, MobMachineState, MobMachineTransition,
    MobMemberState, MobPhase, PeerAddress, PeerId, PeerName, PeerSigningKey,
    PlacedCompletionLifecycleIntentKind, ResumeMemberBinding, SessionId, SpawnPolicyRuntimeMode,
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

/// The member's comms endpoint, registered after its spawn as the actor does.
/// A local wake closes Live only with a registered endpoint.
fn register_member_endpoint(authority: &mut MobMachineAuthority, identity_name: &str) {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::RegisterMemberPeer {
            agent_identity: identity(identity_name),
            agent_runtime_id: runtime_id(identity_name, GENERATION),
            generation: Generation(GENERATION),
            fence_token: FenceToken(FENCE),
            peer_endpoint: MemberPeerEndpoint::new(
                PeerName(identity_name.to_string()),
                PeerId(format!("peer-{identity_name}")),
                PeerAddress(format!("inproc://{identity_name}")),
                PeerSigningKey([9; 32]),
            ),
        },
    )
    .expect("the spawned member registers its endpoint");
}

/// The drift arm: the machine believed the member Live and a probe observed
/// its materialization gone with a durable snapshot present. It opens exactly
/// one wake attempt (Live -> Waking) and returns the attempt id the shell
/// fences every resolution with.
fn authorize_revival(authority: &mut MobMachineAuthority, identity_name: &str) -> u64 {
    let transition = authority
        .apply_signal(MobMachineSignal::ClassifyMemberLiveMaterialization {
            agent_identity: identity(identity_name),
            observation: MemberLiveMaterializationObservationKind::DurableSnapshotPresent,
            reason: "live materialization missing at dispatch".to_string(),
        })
        .expect("a durable snapshot must authorize exactly one revival attempt");
    let attempt = transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            MobMachineEffect::MemberLiveMaterializationClassified {
                verdict: MemberRevivalVerdictKind::ReviveAuthorized,
                attempt,
                ..
            } => *attempt,
            _ => None,
        })
        .expect("revival authorization is the machine-owned verdict this fixture depends on");
    assert_eq!(
        authority
            .state()
            .member_wake_attempts
            .get(&identity(identity_name)),
        Some(&attempt),
        "authorization must open the wake attempt it reports",
    );
    assert_eq!(
        authority
            .state()
            .member_materialization
            .get(&identity(identity_name)),
        Some(&MemberMaterializationPhase::Waking),
        "an authorized revival is a Waking member",
    );
    attempt
}

fn feedback_verdicts(transition: &MobMachineTransition) -> Vec<MemberWakeFeedbackVerdict> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::MemberWakeFeedbackResolved { verdict, .. } => Some(*verdict),
            _ => None,
        })
        .collect()
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
                ..
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
    register_member_endpoint(&mut authority, MEMBER);
    let attempt = authorize_revival(&mut authority, MEMBER);

    // Step 1 (Bound): the provision registered the runtime; the machine asks
    // the consumer to bind the member's placement tuple before its drain
    // admits any work. The attempt stays open.
    let bound = authority
        .apply_signal(MobMachineSignal::ResolveMemberRevivalBound {
            agent_identity: identity(MEMBER),
            attempt,
        })
        .expect("the Bound step of a rebindable local wake applies");
    assert_eq!(
        binding_requests(&bound),
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
    assert_eq!(
        feedback_verdicts(&bound),
        vec![MemberWakeFeedbackVerdict::Applied]
    );
    assert_eq!(
        authority
            .state()
            .member_wake_attempts
            .get(&identity(MEMBER)),
        Some(&attempt),
        "the attempt stays open until readiness and topology settled",
    );

    // Step 2 (Ready): the attempt closes Live; the binding request belongs to
    // the Bound step only.
    let ready = authority
        .apply_signal(MobMachineSignal::ResolveMemberRevivalSucceeded {
            agent_identity: identity(MEMBER),
            attempt,
        })
        .expect("a bound local wake resolves Live");
    assert!(binding_requests(&ready).is_empty());
    assert_eq!(
        feedback_verdicts(&ready),
        vec![MemberWakeFeedbackVerdict::Applied]
    );
    assert_eq!(
        authority
            .state()
            .member_materialization
            .get(&identity(MEMBER)),
        Some(&MemberMaterializationPhase::Live),
    );
    assert!(
        !authority
            .state()
            .member_wake_attempts
            .contains_key(&identity(MEMBER)),
        "a resolved revival closes the machine-owned attempt",
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
fn an_open_wake_never_outlives_its_session_binding() {
    let mut authority = MobMachineAuthority::new();
    authorize_spawn_profile(&mut authority, MEMBER);
    apply_local_spawn_ladder(&mut authority, MEMBER);
    authorize_revival(&mut authority, MEMBER);

    // A retirement or rebind that releases the binding closes the attempt in
    // the same transition, so the Bound step never has to name a missing
    // tuple: a Waking member without a session binding is not a reachable,
    // recoverable shape.
    let mut unbindable = authority.state().clone();
    assert!(
        unbindable
            .member_session_bindings
            .remove(&identity(MEMBER))
            .is_some(),
        "the fixture must actually remove the session binding it is testing for",
    );
    assert!(
        MobMachineAuthority::recover_from_state(unbindable).is_err(),
        "a residency phase exists only for a session-bound member \
         (materialization_domain_is_session_bound_membership)",
    );
}

#[test]
fn feedback_without_an_open_attempt_is_a_typed_stale_no_op() {
    let mut authority = MobMachineAuthority::new();
    authorize_spawn_profile(&mut authority, MEMBER);
    apply_local_spawn_ladder(&mut authority, MEMBER);
    let before = authority.state().clone();

    for signal in [
        MobMachineSignal::ResolveMemberRevivalBound {
            agent_identity: identity(MEMBER),
            attempt: 1,
        },
        MobMachineSignal::ResolveMemberRevivalSucceeded {
            agent_identity: identity(MEMBER),
            attempt: 1,
        },
    ] {
        let stale = authority
            .apply_signal(signal)
            .expect("late or unsolicited wake feedback is a total machine verdict");
        assert!(
            binding_requests(&stale).is_empty(),
            "feedback for an attempt the machine never opened must never mint a binding request",
        );
        assert_eq!(
            feedback_verdicts(&stale),
            vec![MemberWakeFeedbackVerdict::StaleAttempt]
        );
        assert_eq!(
            authority.state(),
            &before,
            "a stale verdict changes nothing"
        );
    }
}

fn recreation_completed(attempt: u64) -> MobMachineSignal {
    MobMachineSignal::ResolveRecreatedMemberSessionSucceeded {
        agent_identity: identity(MEMBER),
        agent_runtime_id: runtime_id(MEMBER, GENERATION),
        fence_token: FenceToken(FENCE),
        generation: Generation(GENERATION),
        bridge_session_id: session_id("recreated-session"),
        attempt,
    }
}

/// A wake whose nonpersistent session was recreated: the drift arm opened the
/// attempt, then the fresh session's binding was recovered (the recovery arm
/// preserves the residency phase).
fn recreated_member_authority() -> (MobMachineAuthority, u64) {
    let mut authority = MobMachineAuthority::new();
    authorize_spawn_profile(&mut authority, MEMBER);
    apply_local_spawn_ladder(&mut authority, MEMBER);
    let attempt = authorize_revival(&mut authority, MEMBER);
    authority
        .apply_signal(MobMachineSignal::RecoverMemberSessionBinding {
            agent_identity: identity(MEMBER),
            agent_runtime_id: runtime_id(MEMBER, GENERATION),
            bridge_session_id: session_id("recreated-session"),
            replacing: Some(session_id(BRIDGE_SESSION)),
        })
        .expect("fresh recreation recovers the new session binding first");
    assert_eq!(
        authority
            .state()
            .member_materialization
            .get(&identity(MEMBER)),
        Some(&MemberMaterializationPhase::Waking),
        "recovering the recreated session's binding never overwrites the phase",
    );
    (authority, attempt)
}

fn recreated_binding_request() -> Vec<(
    AgentIdentity,
    AgentRuntimeId,
    FenceToken,
    Option<Generation>,
    SessionId,
)> {
    vec![(
        identity(MEMBER),
        runtime_id(MEMBER, GENERATION),
        FenceToken(FENCE),
        Some(Generation(GENERATION)),
        session_id("recreated-session"),
    )]
}

#[test]
fn recreated_member_completion_emits_only_its_exact_current_binding() {
    let (mut authority, attempt) = recreated_member_authority();
    let first = authority
        .apply_signal(recreation_completed(attempt))
        .expect("an exact recreation is the wake's Bound step");
    let bound = authority.state().clone();
    assert!(bound.member_wake_bound.contains(&identity(MEMBER)));
    assert_eq!(
        bound.member_wake_attempts.get(&identity(MEMBER)),
        Some(&attempt),
        "the attempt stays open until ResolveMemberRevivalSucceeded closes it Live",
    );
    let again = authority
        .apply_signal(recreation_completed(attempt))
        .expect("an exact successful recreation is idempotent");
    for transition in [&first, &again] {
        assert_eq!(binding_requests(transition), recreated_binding_request());
        assert_eq!(
            feedback_verdicts(transition),
            vec![MemberWakeFeedbackVerdict::Applied]
        );
    }
    assert_eq!(authority.state(), &bound);
}

#[test]
fn recreated_member_completion_refuses_stale_receipt_fields_without_effects() {
    let (baseline, attempt) = recreated_member_authority();
    let before = baseline.state().clone();
    for (label, member, runtime, fence, generation, session) in [
        (
            "runtime",
            MEMBER,
            "other-runtime",
            FENCE,
            GENERATION,
            "recreated-session",
        ),
        (
            "fence",
            MEMBER,
            MEMBER,
            FENCE + 1,
            GENERATION,
            "recreated-session",
        ),
        (
            "generation",
            MEMBER,
            MEMBER,
            FENCE,
            GENERATION + 1,
            "recreated-session",
        ),
        ("session", MEMBER, MEMBER, FENCE, GENERATION, BRIDGE_SESSION),
    ] {
        let mut authority = MobMachineAuthority::recover_from_state(before.clone())
            .expect("recover exact baseline");
        authority
            .apply_signal(MobMachineSignal::ResolveRecreatedMemberSessionSucceeded {
                agent_identity: identity(member),
                agent_runtime_id: runtime_id(runtime, GENERATION),
                fence_token: FenceToken(fence),
                generation: Generation(generation),
                bridge_session_id: session_id(session),
                attempt,
            })
            .unwrap_err();
        assert_eq!(
            authority.state(),
            &before,
            "stale {label} changed authority"
        );
    }

    // Another identity has no open attempt: its feedback is the typed Stale
    // verdict, never a binding request.
    let mut authority =
        MobMachineAuthority::recover_from_state(before.clone()).expect("recover exact baseline");
    let stale = authority
        .apply_signal(MobMachineSignal::ResolveRecreatedMemberSessionSucceeded {
            agent_identity: identity("other-member"),
            agent_runtime_id: runtime_id(MEMBER, GENERATION),
            fence_token: FenceToken(FENCE),
            generation: Generation(GENERATION),
            bridge_session_id: session_id("recreated-session"),
            attempt,
        })
        .expect("feedback for an identity with no open attempt is a total verdict");
    assert!(binding_requests(&stale).is_empty());
    assert_eq!(
        feedback_verdicts(&stale),
        vec![MemberWakeFeedbackVerdict::StaleAttempt]
    );
    assert_eq!(
        authority.state(),
        &before,
        "stale identity changed authority"
    );
}

#[test]
fn recreated_member_completion_refuses_obsolete_authority_without_effects() {
    let (baseline, attempt) = recreated_member_authority();
    let baseline = baseline.state().clone();
    type AuthorityMutation = fn(&mut MobMachineState);

    // The attempt is still open, but the incarnation it names is no longer
    // the exact current, live, unquiesced one: refused, nothing changes.
    let refused: &[(&str, AuthorityMutation)] = &[
        ("quiescing", |state| {
            state.placed_completion_lifecycle_quiescing = true;
            state.placed_completion_lifecycle_intent =
                Some(PlacedCompletionLifecycleIntentKind::Stop);
        }),
        ("not live", |state| {
            state
                .live_runtime_ids
                .remove(&runtime_id(MEMBER, GENERATION));
        }),
        ("retirement pending", |state| {
            state.runtime_retire_pending_sessions.insert(
                runtime_id(MEMBER, GENERATION),
                session_id("recreated-session"),
            );
        }),
        ("session rotated", |state| {
            state
                .member_session_bindings
                .insert(identity(MEMBER), session_id("successor"));
        }),
        ("runtime fence absent", |state| {
            state
                .runtime_fence_tokens
                .remove(&runtime_id(MEMBER, GENERATION));
        }),
        ("runtime fence changed", |state| {
            state
                .runtime_fence_tokens
                .insert(runtime_id(MEMBER, GENERATION), FenceToken(FENCE + 1));
        }),
    ];
    for &(label, mutate) in refused {
        let mut before = baseline.clone();
        mutate(&mut before);
        let mut authority = MobMachineAuthority::recover_from_state(before.clone())
            .unwrap_or_else(|error| panic!("{label} fixture must recover: {error}"));
        authority
            .apply_signal(recreation_completed(attempt))
            .unwrap_err();
        assert_eq!(authority.state(), &before, "{label} changed authority");
    }

    // The competing transition already closed the attempt (a Stop moves the
    // member to Dormant, a retirement start or a Broken verdict removes its
    // phase, a newer attempt supersedes it): the late completion is the typed
    // Stale verdict, with no binding request and no state change.
    let closed: &[(&str, AuthorityMutation)] = &[
        ("stopped", |state| {
            state.lifecycle_phase = MobPhase::Stopped;
            // A Stopped mob holds member run starts (machine invariant).
            state.member_run_starts_held = true;
            state
                .member_materialization
                .insert(identity(MEMBER), MemberMaterializationPhase::Dormant);
            state.member_wake_attempts.remove(&identity(MEMBER));
            state.member_wake_bound.remove(&identity(MEMBER));
        }),
        ("retiring", |state| {
            state
                .member_state_markers
                .insert(runtime_id(MEMBER, GENERATION), MobMemberState::Retiring);
            state.member_materialization.remove(&identity(MEMBER));
            state.member_wake_attempts.remove(&identity(MEMBER));
            state.member_wake_bound.remove(&identity(MEMBER));
        }),
        ("broken", |state| {
            state
                .member_restore_failures
                .insert(identity(MEMBER), "restore failed".into());
            state.member_materialization.remove(&identity(MEMBER));
            state.member_wake_attempts.remove(&identity(MEMBER));
            state.member_wake_bound.remove(&identity(MEMBER));
        }),
        ("superseded", |state| {
            state.member_wake_attempt_sequence += 1;
            let newer = state.member_wake_attempt_sequence;
            state.member_wake_attempts.insert(identity(MEMBER), newer);
        }),
    ];
    for &(label, mutate) in closed {
        let mut before = baseline.clone();
        mutate(&mut before);
        let mut authority = MobMachineAuthority::recover_from_state(before.clone())
            .unwrap_or_else(|error| panic!("{label} fixture must recover: {error}"));
        let stale = authority
            .apply_signal(recreation_completed(attempt))
            .unwrap_or_else(|error| panic!("{label}: late feedback is a total verdict: {error}"));
        assert!(
            binding_requests(&stale).is_empty(),
            "{label} minted a binding"
        );
        assert_eq!(
            feedback_verdicts(&stale),
            vec![MemberWakeFeedbackVerdict::StaleAttempt],
            "{label}"
        );
        assert_eq!(authority.state(), &before, "{label} changed authority");
    }

    // A Waking member without a session binding is not a recoverable shape.
    let mut unbound = baseline.clone();
    unbound.member_session_bindings.remove(&identity(MEMBER));
    assert!(MobMachineAuthority::recover_from_state(unbound).is_err());

    // An explicit resume reserves the member through this same attempt (it
    // joined or opened it), so the recreation completes its Bound step.
    let mut resuming = baseline;
    resuming.explicit_resume_member_work.insert(
        identity(MEMBER),
        ResumeMemberBinding {
            agent_runtime_id: runtime_id(MEMBER, GENERATION),
            fence_token: FenceToken(FENCE),
            session_id: session_id("recreated-session"),
            definition_epoch: 0,
        },
    );
    let mut authority = MobMachineAuthority::recover_from_state(resuming)
        .expect("an explicit resume over an open attempt is a recoverable shape");
    let applied = authority
        .apply_signal(recreation_completed(attempt))
        .expect("the explicit resume's own attempt completes its Bound step");
    assert_eq!(binding_requests(&applied), recreated_binding_request());
}

#[test]
fn recreated_member_recovery_refuses_lifecycle_intent_without_quiescence() {
    let mut state = recreated_member_authority().0.state().clone();
    assert!(!state.placed_completion_lifecycle_quiescing);
    state.placed_completion_lifecycle_intent = Some(PlacedCompletionLifecycleIntentKind::Stop);
    assert!(
        MobMachineAuthority::recover_from_state(state).is_err(),
        "an intent-only snapshot must fail invariant validation before any recreation completion"
    );
}
