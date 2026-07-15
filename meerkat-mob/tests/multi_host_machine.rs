//! Multi-host mobs phase 1 — MobMachine catalog-authority coverage.
//!
//! These tests exercise the DSL authority directly (no actor, no comms), the
//! `member_session_bindings.rs` style, pinning the §6.1/§6.2/§6.5/§15.4/
//! §18.9/§19.L1/L2/L4/L5 machine deltas at the level where they are defined:
//!   * host bind ladder (begin → commit → rebound → revoke), strict epoch
//!     monotonicity, live-endpoint restart truthfulness;
//!   * the remote spawn ladder (denial table, MaterializePending ordering,
//!     ack-guarded membership commit, materialization failure retry);
//!   * flow-step dispatch classification (4-outcome matrix);
//!   * route-install and remote-turn obligation windows (idempotence);
//!   * principal control-scope grants (partition-revalidated revoke; expiry
//!     is data — the machine never reads a clock);
//!   * the subscription third outcome and the SubscribeAll external set;
//!   * member-operator upcall admission (admit + every typed reject cause);
//!   * retirement disposal (typed field; destroy family clears placement);
//!   * a 2-host × 3-member invariant walk.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use meerkat_mob::machines::mob_machine::{
    AgentIdentity, AgentRuntimeId, ControlScope, FenceToken, FlowStepDispatchKind, Generation,
    HostBindPhase, HostId, InputId, LiveWsEndpointUrl, MemberOperatorRejectKind,
    MemberPeerEndpoint, MemberSessionDisposal, MobId, MobLifecycleJournalKind, MobMachineAuthority,
    MobMachineEffect, MobMachineInput, MobMachineMutator, MobMachineSignal, MobMachineState,
    MobPhase, MobSpawnMemberAdmissionKind, PeerAddress, PeerId, PeerName, PeerSigningKey,
    PlacedCompletionLifecycleIntentKind, PlacedSpawnId, PrincipalId, RemoteTurnObligation,
    RouteInstallObligation, RouteObligationKind, RunId, SessionId, SpawnExecPhase,
    SpawnPolicyRuntimeMode, StepId, WiringEdge,
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

fn host_id(label: &str) -> HostId {
    HostId(label.to_string())
}

fn profile_material_digest(identity_name: &str, generation: u64) -> String {
    format!("test-profile-digest:{identity_name}:{generation}")
}

fn resolved_spec_digest(identity_name: &str, generation: u64) -> String {
    format!("resolved-spec-digest:{identity_name}:{generation}")
}

fn placed_spawn_id(identity_name: &str, generation: u64) -> PlacedSpawnId {
    PlacedSpawnId(format!("placed-spawn:{identity_name}:{generation}"))
}

fn provision_operation_id(identity_name: &str, generation: u64) -> String {
    format!("provision-operation:{identity_name}:{generation}")
}

fn member_endpoint(identity_name: &str, key_byte: u8) -> MemberPeerEndpoint {
    MemberPeerEndpoint {
        name: PeerName(identity_name.to_string()),
        peer_id: PeerId(format!("peer-{identity_name}")),
        address: PeerAddress(format!("tcp://members/{identity_name}")),
        signing_key: PeerSigningKey([key_byte; 32]),
    }
}

/// Capability shape for a bind/rebind test host.
#[derive(Clone, Copy)]
struct HostCaps {
    durable_sessions: bool,
    autonomous_members: bool,
    tracked_input_cancel: bool,
    memory_store: bool,
    mcp: bool,
    live: bool,
}

impl HostCaps {
    const FULL: Self = Self {
        durable_sessions: true,
        autonomous_members: true,
        tracked_input_cancel: true,
        memory_store: true,
        mcp: true,
        live: true,
    };
}

fn commit_host_bind_input(host: &HostId, epoch: u64, caps: HostCaps) -> MobMachineInput {
    commit_host_bind_input_at_generation(host, epoch, 1, caps)
}

fn commit_host_bind_input_at_generation(
    host: &HostId,
    epoch: u64,
    binding_generation: u64,
    caps: HostCaps,
) -> MobMachineInput {
    MobMachineInput::CommitHostBind {
        host_id: host.clone(),
        pubkey: PeerSigningKey([9; 32]),
        endpoint: PeerAddress(format!("tcp://hosts/{}", host.0)),
        epoch,
        binding_generation,
        protocol_min: 4,
        protocol_max: 4,
        engine_version: "0.7.22-test".to_string(),
        durable_sessions: caps.durable_sessions,
        autonomous_members: caps.autonomous_members,
        hard_cancel_member: true,
        tracked_input_cancel: caps.tracked_input_cancel,
        memory_store: caps.memory_store,
        mcp: caps.mcp,
        resolvable_providers: BTreeSet::from(["anthropic".to_string()]),
        approval_forwarding: false,
        live_endpoint: caps
            .live
            .then(|| LiveWsEndpointUrl(format!("wss://hosts/{}/live", host.0))),
    }
}

fn host_rebound_input_with_tracked_contract(
    host: &HostId,
    epoch: u64,
    protocol_min: u64,
    protocol_max: u64,
    durable_sessions: bool,
    tracked_input_cancel: bool,
) -> MobMachineInput {
    MobMachineInput::HostRebound {
        host_id: host.clone(),
        epoch,
        binding_generation: 1,
        protocol_min,
        protocol_max,
        engine_version: "0.7.23-test".to_string(),
        durable_sessions,
        autonomous_members: true,
        hard_cancel_member: true,
        tracked_input_cancel,
        memory_store: true,
        mcp: true,
        resolvable_providers: BTreeSet::new(),
        approval_forwarding: false,
        live_endpoint: None,
    }
}

fn refresh_host_capabilities_input_with_tracked_contract(
    host: &HostId,
    protocol_min: u64,
    protocol_max: u64,
    durable_sessions: bool,
    tracked_input_cancel: bool,
) -> MobMachineInput {
    MobMachineInput::RefreshHostCapabilities {
        host_id: host.clone(),
        epoch: 1,
        binding_generation: 1,
        protocol_min,
        protocol_max,
        engine_version: "0.7.23-test".to_string(),
        durable_sessions,
        autonomous_members: true,
        hard_cancel_member: true,
        tracked_input_cancel,
        memory_store: true,
        mcp: true,
        resolvable_providers: BTreeSet::new(),
        approval_forwarding: false,
        live_endpoint: None,
    }
}

fn bind_host(authority: &mut MobMachineAuthority, host: &HostId, epoch: u64, caps: HostCaps) {
    bind_host_at_generation(authority, host, epoch, 1, caps);
}

fn bind_host_at_generation(
    authority: &mut MobMachineAuthority,
    host: &HostId,
    epoch: u64,
    binding_generation: u64,
    caps: HostCaps,
) {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::BeginHostBind {
            host_id: host.clone(),
            expected_endpoint: PeerAddress(format!("tcp://hosts/{}", host.0)),
            binding_generation,
        },
    )
    .expect("BeginHostBind must open the bind window");
    MobMachineMutator::apply(
        authority,
        commit_host_bind_input_at_generation(host, epoch, binding_generation, caps),
    )
    .expect("CommitHostBind must commit the requested bind");
}

fn bind_owner_bridge(authority: &mut MobMachineAuthority) {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::BindOwnerBridgeSession {
            bridge_session_id: session_id("owner-bridge"),
            destroy_on_owner_archive: false,
            implicit_delegation_mob: false,
        },
    )
    .expect("owner bridge session must bind");
}

fn begin_lifecycle_quiesce(
    authority: &mut MobMachineAuthority,
    intent: PlacedCompletionLifecycleIntentKind,
) {
    MobMachineMutator::apply(
        authority,
        MobMachineInput::BeginPlacedCompletionLifecycleQuiesce { intent },
    )
    .expect("lifecycle fixture must record the independent completion-drain intent");
}

fn authorize_spawn_profile(
    authority: &mut MobMachineAuthority,
    identity_name: &str,
    generation: u64,
    resolved: Option<String>,
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
            resolved_spec_digest: resolved,
        },
    )
    .expect("spawn profile material must be authorized");
}

/// Local BeginSpawnExec: placement None keeps every denial guard vacuous.
fn begin_spawn_local(
    identity_name: &str,
    generation: u64,
    bridge_sid: Option<SessionId>,
    runtime_mode: SpawnPolicyRuntimeMode,
) -> MobMachineInput {
    MobMachineInput::BeginSpawnExec {
        agent_identity: identity(identity_name),
        agent_runtime_id: runtime_id(identity_name, generation),
        fence_token: FenceToken(generation + 1),
        generation: Generation(generation),
        profile_material_digest: profile_material_digest(identity_name, generation),
        external_addressable: false,
        runtime_mode,
        bridge_session_id: bridge_sid,
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
    }
}

/// All-clean remote BeginSpawnExec targeting `host`.
fn begin_spawn_remote(
    identity_name: &str,
    generation: u64,
    host: &HostId,
    runtime_mode: SpawnPolicyRuntimeMode,
) -> MobMachineInput {
    MobMachineInput::BeginSpawnExec {
        agent_identity: identity(identity_name),
        agent_runtime_id: runtime_id(identity_name, generation),
        fence_token: FenceToken(generation + 1),
        generation: Generation(generation),
        profile_material_digest: profile_material_digest(identity_name, generation),
        external_addressable: false,
        runtime_mode,
        bridge_session_id: None,
        replacing: None,
        placement: Some(host.clone()),
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
        placed_spawn_id: Some(placed_spawn_id(identity_name, generation)),
        placed_provision_operation_id: Some(provision_operation_id(identity_name, generation)),
        placed_operation_owner_session_id: Some(session_id("owner-bridge")),
        effective_profile_override_present: false,
        effective_model_override_present: false,
    }
}

fn commit_membership_local(
    identity_name: &str,
    generation: u64,
    bridge_sid: Option<SessionId>,
    runtime_mode: SpawnPolicyRuntimeMode,
) -> MobMachineInput {
    MobMachineInput::CommitSpawnMembership {
        agent_identity: identity(identity_name),
        agent_runtime_id: runtime_id(identity_name, generation),
        fence_token: FenceToken(generation + 1),
        generation: Generation(generation),
        profile_material_digest: profile_material_digest(identity_name, generation),
        external_addressable: false,
        runtime_mode,
        bridge_session_id: bridge_sid,
        replacing: None,
        member_peer_endpoint: None,
        spec_digest_echo: None,
        ack_engine_version: None,
        placed_spawn_id: None,
        provision_operation_id: None,
    }
}

fn commit_membership_remote(
    identity_name: &str,
    generation: u64,
    ack_session: &SessionId,
    spec_digest_echo: Option<String>,
    ack_engine_version: &str,
) -> MobMachineInput {
    MobMachineInput::CommitSpawnMembership {
        agent_identity: identity(identity_name),
        agent_runtime_id: runtime_id(identity_name, generation),
        fence_token: FenceToken(generation + 1),
        generation: Generation(generation),
        profile_material_digest: profile_material_digest(identity_name, generation),
        external_addressable: false,
        runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
        bridge_session_id: Some(ack_session.clone()),
        replacing: None,
        member_peer_endpoint: Some(member_endpoint(identity_name, 3)),
        spec_digest_echo,
        ack_engine_version: Some(ack_engine_version.to_string()),
        placed_spawn_id: Some(placed_spawn_id(identity_name, generation)),
        provision_operation_id: Some(provision_operation_id(identity_name, generation)),
    }
}

/// Seed a fully-committed LOCAL member through the machine ladder.
fn seed_local_member(
    authority: &mut MobMachineAuthority,
    identity_name: &str,
    generation: u64,
    runtime_mode: SpawnPolicyRuntimeMode,
) -> SessionId {
    let bridge = session_id(&format!("bridge-{identity_name}-gen{generation}"));
    authorize_spawn_profile(authority, identity_name, generation, None);
    MobMachineMutator::apply(
        authority,
        begin_spawn_local(
            identity_name,
            generation,
            Some(bridge.clone()),
            runtime_mode,
        ),
    )
    .expect("local BeginSpawnExec must open");
    MobMachineMutator::apply(
        authority,
        commit_membership_local(
            identity_name,
            generation,
            Some(bridge.clone()),
            runtime_mode,
        ),
    )
    .expect("local CommitSpawnMembership must commit");
    MobMachineMutator::apply(
        authority,
        MobMachineInput::CommitSpawnActivation {
            agent_identity: identity(identity_name),
        },
    )
    .expect("local CommitSpawnActivation must settle");
    bridge
}

/// Seed a fully-committed REMOTE member (host must already be bound and the
/// owner bridge anchored).
fn seed_remote_member(
    authority: &mut MobMachineAuthority,
    identity_name: &str,
    generation: u64,
    host: &HostId,
    runtime_mode: SpawnPolicyRuntimeMode,
) -> SessionId {
    let resolved = resolved_spec_digest(identity_name, generation);
    authorize_spawn_profile(authority, identity_name, generation, Some(resolved.clone()));
    MobMachineMutator::apply(
        authority,
        begin_spawn_remote(identity_name, generation, host, runtime_mode),
    )
    .expect("remote BeginSpawnExec must enter MaterializePending");
    let ack_session = session_id(&format!("member-host-session-{identity_name}"));
    let commit = MobMachineMutator::apply(
        authority,
        MobMachineInput::CommitSpawnMembership {
            agent_identity: identity(identity_name),
            agent_runtime_id: runtime_id(identity_name, generation),
            fence_token: FenceToken(generation + 1),
            generation: Generation(generation),
            profile_material_digest: profile_material_digest(identity_name, generation),
            external_addressable: false,
            runtime_mode,
            bridge_session_id: Some(ack_session.clone()),
            replacing: None,
            member_peer_endpoint: Some(member_endpoint(identity_name, 3)),
            spec_digest_echo: Some(resolved),
            ack_engine_version: Some("0.7.22-test".to_string()),
            placed_spawn_id: Some(placed_spawn_id(identity_name, generation)),
            provision_operation_id: Some(provision_operation_id(identity_name, generation)),
        },
    )
    .expect("remote CommitSpawnMembership must commit from the ack");
    assert!(
        !commit
            .effects()
            .iter()
            .any(|effect| matches!(effect, MobMachineEffect::RequestRuntimeBinding { .. })),
        "the member host owns the runtime binding — no local binding request for a remote commit",
    );
    MobMachineMutator::apply(
        authority,
        MobMachineInput::CommitSpawnActivation {
            agent_identity: identity(identity_name),
        },
    )
    .expect("remote CommitSpawnActivation must settle");
    ack_session
}

fn spawn_admission(
    transition: &meerkat_mob::machines::mob_machine::MobMachineTransition,
) -> Option<MobSpawnMemberAdmissionKind> {
    transition.effects().iter().find_map(|effect| match effect {
        MobMachineEffect::SpawnMemberAdmissionResolved { admission } => Some(*admission),
        _ => None,
    })
}

/// Manual re-statement of the new multi-host invariants (the kernel-test
/// analog of the TLC checks).
fn check_multi_host_invariants(state: &MobMachineState) {
    for (id, host) in &state.member_placement {
        assert!(
            !state.host_bind_phase.contains_key(host)
                || state.host_bind_phase.get(host) == Some(&HostBindPhase::Bound),
            "placement for {id:?} targets half-bound host {host:?}",
        );
        if state.member_session_bindings.contains_key(id) {
            assert!(
                state.member_peer_endpoints.contains_key(id),
                "remote committed member {id:?} is missing its ack peer endpoint",
            );
        }
    }
    if !state.member_placement.is_empty() {
        assert!(
            state.owner_bridge_session_id.is_some(),
            "placed members require the owner bridge session",
        );
    }
    for host in state.host_live_endpoints.keys() {
        assert_eq!(
            state.host_bind_phase.get(host),
            Some(&HostBindPhase::Bound),
            "live endpoint recorded for a non-bound host {host:?}",
        );
    }
    for host in &state.mob_hosts {
        assert!(state.host_authority_epochs.contains_key(host));
        assert!(state.host_durable_sessions.contains_key(host));
        assert!(state.host_engine_versions.contains_key(host));
        assert!(state.host_resolvable_providers.contains_key(host));
    }
    for host in state.host_authority_epochs.keys() {
        assert!(state.mob_hosts.contains(host));
    }
}

// ==========================================================================
// Host bind ladder (§6.1)
// ==========================================================================

#[test]
fn host_bind_ladder_records_all_host_facts() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");

    let begin = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginHostBind {
            host_id: host.clone(),
            expected_endpoint: PeerAddress("tcp://hosts/host-b".to_string()),
            binding_generation: 1,
        },
    )
    .expect("fresh BeginHostBind accepted");
    assert!(
        begin.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::RequestHostBind { host_id, .. } if host_id == &host
        )),
        "BeginHostBind must emit the bind handoff",
    );
    assert_eq!(
        authority.state().host_bind_phase.get(&host),
        Some(&HostBindPhase::Requested),
    );
    assert!(
        !authority.state().mob_hosts.contains(&host),
        "a Requested-only host is not a mob host yet",
    );

    // Idempotent re-request while the window is open.
    let rerequest = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginHostBind {
            host_id: host.clone(),
            expected_endpoint: PeerAddress("tcp://hosts/host-b".to_string()),
            binding_generation: 1,
        },
    )
    .expect("re-requesting an open bind window is idempotent");
    assert!(
        rerequest
            .effects()
            .iter()
            .any(|effect| matches!(effect, MobMachineEffect::RequestHostBind { .. })),
    );

    let commit = MobMachineMutator::apply(
        &mut authority,
        commit_host_bind_input(&host, 1, HostCaps::FULL),
    )
    .expect("CommitHostBind accepted for a requested window");
    assert!(commit.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::HostRegistered { host_id, epoch: 1, .. } if host_id == &host
    )));

    let state = authority.state();
    assert!(state.mob_hosts.contains(&host));
    assert_eq!(
        state.host_bind_phase.get(&host),
        Some(&HostBindPhase::Bound)
    );
    assert_eq!(state.host_authority_epochs.get(&host), Some(&1));
    assert_eq!(
        state.host_public_keys.get(&host),
        Some(&PeerSigningKey([9; 32]))
    );
    assert_eq!(
        state.host_endpoints.get(&host),
        Some(&PeerAddress("tcp://hosts/host-b".to_string())),
    );
    assert_eq!(state.host_protocol_min.get(&host), Some(&4));
    assert_eq!(state.host_protocol_max.get(&host), Some(&4));
    assert_eq!(
        state.host_engine_versions.get(&host),
        Some(&"0.7.22-test".to_string()),
    );
    assert_eq!(state.host_durable_sessions.get(&host), Some(&true));
    assert_eq!(state.host_autonomous_members.get(&host), Some(&true));
    assert_eq!(state.host_hard_cancel_member.get(&host), Some(&true));
    assert_eq!(state.host_memory_store.get(&host), Some(&true));
    assert_eq!(state.host_mcp.get(&host), Some(&true));
    assert_eq!(state.host_approval_forwarding.get(&host), Some(&false));
    assert!(
        state
            .host_resolvable_providers
            .get(&host)
            .expect("providers recorded")
            .contains("anthropic"),
    );
    assert_eq!(
        state.host_live_endpoints.get(&host),
        Some(&LiveWsEndpointUrl("wss://hosts/host-b/live".to_string())),
    );
    check_multi_host_invariants(state);

    // A committed bind cannot be re-opened without a revoke.
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::BeginHostBind {
                host_id: host,
                expected_endpoint: PeerAddress("tcp://hosts/host-b".to_string()),
                binding_generation: 1,
            },
        )
        .is_err(),
        "BeginHostBind over a Bound host must be rejected",
    );
}

#[test]
fn recovery_rejects_stray_host_capability_keys() {
    let authority = MobMachineAuthority::new();
    let mut state = authority.state().clone();
    state.host_protocol_min.insert(host_id("ghost-host"), 1);
    assert!(
        MobMachineAuthority::recover_from_state(state).is_err(),
        "every host capability map must have exactly the mob_hosts key set"
    );
}

#[test]
fn host_rebind_is_strictly_epoch_monotonic_and_redeclares_capabilities() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_host(&mut authority, &host, 3, HostCaps::FULL);

    let rebound_input = |epoch: u64, live: bool| MobMachineInput::HostRebound {
        host_id: host.clone(),
        epoch,
        binding_generation: 1,
        protocol_min: 4,
        protocol_max: 4,
        engine_version: "0.7.23-test".to_string(),
        durable_sessions: true,
        autonomous_members: false,
        hard_cancel_member: true,
        tracked_input_cancel: true,
        memory_store: true,
        mcp: true,
        resolvable_providers: BTreeSet::new(),
        approval_forwarding: false,
        live_endpoint: live.then(|| LiveWsEndpointUrl("wss://hosts/host-b/live2".to_string())),
    };

    // Same epoch and lower epoch are typed machine rejections.
    assert!(
        MobMachineMutator::apply(&mut authority, rebound_input(3, true)).is_err(),
        "rebind at the recorded epoch must not fire",
    );
    assert!(
        MobMachineMutator::apply(&mut authority, rebound_input(2, true)).is_err(),
        "rebind below the recorded epoch must not fire",
    );
    assert_eq!(authority.state().host_authority_epochs.get(&host), Some(&3));

    // A strictly higher epoch WITHOUT the live endpoint clears it (restart
    // truthfulness: capability is re-declared each bind).
    let rebound = MobMachineMutator::apply(&mut authority, rebound_input(4, false))
        .expect("strictly higher epoch rebind accepted");
    assert!(rebound.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::HostReboundRecorded { host_id, epoch: 4, .. } if host_id == &host
    )));
    let state = authority.state();
    assert_eq!(state.host_authority_epochs.get(&host), Some(&4));
    assert_eq!(state.host_autonomous_members.get(&host), Some(&false));
    assert_eq!(
        state.host_engine_versions.get(&host),
        Some(&"0.7.23-test".to_string()),
    );
    assert!(
        !state.host_live_endpoints.contains_key(&host),
        "rebind without the live endpoint must clear the entry",
    );
    check_multi_host_invariants(state);
}

#[test]
fn autonomous_placements_fence_the_full_tracked_capability_contract() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    seed_remote_member(
        &mut authority,
        "remote-alpha",
        0,
        &host,
        SpawnPolicyRuntimeMode::AutonomousHost,
    );

    for input in [
        host_rebound_input_with_tracked_contract(&host, 2, 4, 4, false, true),
        host_rebound_input_with_tracked_contract(&host, 2, 4, 4, true, false),
        host_rebound_input_with_tracked_contract(&host, 2, 5, 5, true, true),
        host_rebound_input_with_tracked_contract(&host, 2, 3, 3, true, true),
        refresh_host_capabilities_input_with_tracked_contract(&host, 4, 4, false, true),
        refresh_host_capabilities_input_with_tracked_contract(&host, 4, 4, true, false),
        refresh_host_capabilities_input_with_tracked_contract(&host, 5, 5, true, true),
        refresh_host_capabilities_input_with_tracked_contract(&host, 3, 3, true, true),
    ] {
        assert!(
            MobMachineMutator::apply(&mut authority, input).is_err(),
            "an active placed carrier must retain its durable tracked V4 route"
        );
    }
    let mut rebound_without_autonomous =
        host_rebound_input_with_tracked_contract(&host, 2, 4, 4, true, true);
    if let MobMachineInput::HostRebound {
        autonomous_members, ..
    } = &mut rebound_without_autonomous
    {
        *autonomous_members = false;
    }
    assert!(
        MobMachineMutator::apply(&mut authority, rebound_without_autonomous).is_err(),
        "an autonomous placement cannot lose autonomous_members on rebind"
    );
    let mut refresh_without_autonomous =
        refresh_host_capabilities_input_with_tracked_contract(&host, 4, 4, true, true);
    if let MobMachineInput::RefreshHostCapabilities {
        autonomous_members, ..
    } = &mut refresh_without_autonomous
    {
        *autonomous_members = false;
    }
    assert!(
        MobMachineMutator::apply(&mut authority, refresh_without_autonomous).is_err(),
        "an autonomous placement cannot lose autonomous_members on refresh"
    );
    assert_eq!(authority.state().host_authority_epochs.get(&host), Some(&1));
    assert_eq!(
        authority.state().host_durable_sessions.get(&host),
        Some(&true)
    );
    assert_eq!(
        authority.state().host_tracked_input_cancel.get(&host),
        Some(&true)
    );
    assert_eq!(authority.state().host_protocol_min.get(&host), Some(&4));
    assert_eq!(authority.state().host_protocol_max.get(&host), Some(&4));

    MobMachineMutator::apply(
        &mut authority,
        refresh_host_capabilities_input_with_tracked_contract(&host, 4, 4, true, true),
    )
    .expect("a capability refresh that preserves the tracked V4 contract is allowed");

    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host.clone(),
            binding_generation: 1,
        },
    )
    .expect("revoke retains the dormant placed carrier for replacement binding");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginHostBind {
            host_id: host.clone(),
            expected_endpoint: PeerAddress("tcp://hosts/host-b".to_string()),
            binding_generation: 2,
        },
    )
    .expect("replacement bind request opens for the retained placement");
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            commit_host_bind_input_at_generation(
                &host,
                2,
                2,
                HostCaps {
                    tracked_input_cancel: false,
                    ..HostCaps::FULL
                },
            ),
        )
        .is_err(),
        "replacement bind cannot downgrade a retained placed carrier"
    );
}

#[test]
fn turn_driven_placement_without_tracked_custody_may_degrade_capabilities() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-ephemeral-turn-driven");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    seed_remote_member(
        &mut authority,
        "remote-alpha",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );

    MobMachineMutator::apply(
        &mut authority,
        refresh_host_capabilities_input_with_tracked_contract(&host, 3, 3, false, false),
    )
    .expect("ordinary placement without custody does not invent a tracked-turn requirement");
    assert_eq!(
        authority.state().host_durable_sessions.get(&host),
        Some(&false)
    );
    assert_eq!(
        authority.state().host_tracked_input_cancel.get(&host),
        Some(&false)
    );
}

#[test]
fn pending_autonomous_materialization_fences_host_capability_downgrade() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-pending-autonomous");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    authorize_spawn_profile(
        &mut authority,
        "pending-auto",
        0,
        Some(resolved_spec_digest("pending-auto", 0)),
    );
    MobMachineMutator::apply(
        &mut authority,
        begin_spawn_remote(
            "pending-auto",
            0,
            &host,
            SpawnPolicyRuntimeMode::AutonomousHost,
        ),
    )
    .expect("autonomous remote spawn enters MaterializePending");
    assert!(
        authority
            .state()
            .pending_autonomous_placed_spawns
            .contains(&identity("pending-auto"))
    );

    assert!(
        MobMachineMutator::apply(
            &mut authority,
            refresh_host_capabilities_input_with_tracked_contract(&host, 3, 3, false, false),
        )
        .is_err(),
        "pending autonomous materialization must retain durable tracked V4 host capabilities"
    );
}

#[test]
fn revoke_host_clears_every_host_fact_but_keeps_placement() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    seed_remote_member(
        &mut authority,
        "remote-alpha",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );

    let revoked = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host.clone(),
            binding_generation: 1,
        },
    )
    .expect("RevokeHost accepted for a tracked host");
    assert!(revoked.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::HostRevoked { host_id, .. } if host_id == &host
    )));

    let state = authority.state();
    assert!(!state.mob_hosts.contains(&host));
    assert!(!state.host_bind_phase.contains_key(&host));
    assert!(!state.host_public_keys.contains_key(&host));
    assert!(!state.host_endpoints.contains_key(&host));
    assert!(!state.host_authority_epochs.contains_key(&host));
    assert!(!state.host_durable_sessions.contains_key(&host));
    assert!(!state.host_live_endpoints.contains_key(&host));
    assert!(!state.host_engine_versions.contains_key(&host));
    assert_eq!(
        state.member_placement.get(&identity("remote-alpha")),
        Some(&host),
        "placed members KEEP member_placement across revoke (§9 revival ladder owns re-placement)",
    );
    check_multi_host_invariants(state);
}

#[test]
fn retained_placement_replacement_bind_is_disjoint_until_atomic_commit_or_revoke() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-replacement");
    bind_owner_bridge(&mut authority);
    bind_host_at_generation(&mut authority, &host, 1, 1, HostCaps::FULL);
    seed_remote_member(
        &mut authority,
        "remote-alpha",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host.clone(),
            binding_generation: 1,
        },
    )
    .expect("old host revokes while placement is retained");

    let endpoint = PeerAddress(format!("tcp://hosts/{}", host.0));
    let begin = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginHostBind {
            host_id: host.clone(),
            expected_endpoint: endpoint.clone(),
            binding_generation: 2,
        },
    )
    .expect("replacement bind opens without exposing Requested phase");
    assert!(begin.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::RequestHostBind { host_id, binding_generation: 2, .. }
            if host_id == &host
    )));
    let state = authority.state();
    assert!(!state.mob_hosts.contains(&host));
    assert!(!state.host_bind_phase.contains_key(&host));
    assert_eq!(
        state.replacement_host_bind_endpoints.get(&host),
        Some(&endpoint)
    );
    assert_eq!(
        state.replacement_host_binding_generations.get(&host),
        Some(&2),
    );

    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::SubscribeAgentEvents {
                agent_identity: identity("remote-alpha"),
            },
        )
        .is_err(),
        "remote actions may not emit while replacement binding is only requested",
    );
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginHostBind {
            host_id: host.clone(),
            expected_endpoint: endpoint.clone(),
            binding_generation: 2,
        },
    )
    .expect("exact replacement request retry converges");
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::BeginHostBind {
                host_id: host.clone(),
                expected_endpoint: PeerAddress("tcp://hosts/drift".to_string()),
                binding_generation: 2,
            },
        )
        .is_err(),
        "replacement endpoint drift must reject",
    );
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::BeginHostBind {
                host_id: host.clone(),
                expected_endpoint: endpoint.clone(),
                binding_generation: 3,
            },
        )
        .is_err(),
        "replacement generation drift must reject",
    );

    MobMachineMutator::apply(
        &mut authority,
        commit_host_bind_input_at_generation(&host, 1, 2, HostCaps::FULL),
    )
    .expect("replacement commit atomically restores Bound");
    let state = authority.state();
    assert_eq!(
        state.host_bind_phase.get(&host),
        Some(&HostBindPhase::Bound)
    );
    assert!(state.mob_hosts.contains(&host));
    assert!(!state.replacement_host_bind_endpoints.contains_key(&host));
    assert!(
        !state
            .replacement_host_binding_generations
            .contains_key(&host)
    );

    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host.clone(),
            binding_generation: 2,
        },
    )
    .expect("replacement binding revokes");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginHostBind {
            host_id: host.clone(),
            expected_endpoint: endpoint,
            binding_generation: 3,
        },
    )
    .expect("next replacement request opens");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host.clone(),
            binding_generation: 3,
        },
    )
    .expect("requested replacement revoke clears its exact request tuple");
    assert!(
        !authority
            .state()
            .replacement_host_bind_endpoints
            .contains_key(&host)
    );
    assert!(
        !authority
            .state()
            .replacement_host_binding_generations
            .contains_key(&host)
    );
}

/// FLAG-3: `RecoverHostBinding` (the `RecoverSupervisorAuthority`-style
/// recovery signal fed from a durable `MobHostAuthorityRecord`) restores the
/// FULL bound-host fact set — membership, key, endpoint, epoch, phase, every
/// capability map, and the live endpoint — without any bind ceremony.
#[test]
fn recover_host_binding_signal_restores_all_host_facts() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");

    let recover_signal = |host: &HostId, live: bool| MobMachineSignal::RecoverHostBinding {
        host_id: host.clone(),
        pubkey: PeerSigningKey([9; 32]),
        endpoint: PeerAddress(format!("tcp://hosts/{}", host.0)),
        epoch: 7,
        binding_generation: 1,
        protocol_min: 4,
        protocol_max: 4,
        engine_version: "0.7.22-test".to_string(),
        durable_sessions: true,
        autonomous_members: false,
        hard_cancel_member: true,
        tracked_input_cancel: true,
        memory_store: true,
        mcp: false,
        resolvable_providers: BTreeSet::from(["anthropic".to_string()]),
        approval_forwarding: true,
        live_endpoint: live.then(|| LiveWsEndpointUrl(format!("wss://hosts/{}/live", host.0))),
    };

    let recovered = authority
        .apply_signal(recover_signal(&host, true))
        .expect("recovery of an untracked host must fire");
    assert!(
        recovered.effects().is_empty(),
        "recovery is not a ceremony event — no effects, got {:?}",
        recovered.effects()
    );

    let state = authority.state();
    assert!(state.mob_hosts.contains(&host));
    assert_eq!(
        state.host_bind_phase.get(&host),
        Some(&HostBindPhase::Bound),
        "records exist only for Bound hosts, so recovery restores Bound"
    );
    assert_eq!(
        state.host_public_keys.get(&host),
        Some(&PeerSigningKey([9; 32]))
    );
    assert_eq!(
        state.host_endpoints.get(&host),
        Some(&PeerAddress("tcp://hosts/host-b".to_string()))
    );
    assert_eq!(state.host_authority_epochs.get(&host), Some(&7));
    assert_eq!(state.host_protocol_min.get(&host), Some(&4));
    assert_eq!(state.host_protocol_max.get(&host), Some(&4));
    assert_eq!(
        state.host_engine_versions.get(&host),
        Some(&"0.7.22-test".to_string())
    );
    assert_eq!(state.host_durable_sessions.get(&host), Some(&true));
    assert_eq!(state.host_autonomous_members.get(&host), Some(&false));
    assert_eq!(state.host_hard_cancel_member.get(&host), Some(&true));
    assert_eq!(state.host_memory_store.get(&host), Some(&true));
    assert_eq!(state.host_mcp.get(&host), Some(&false));
    assert_eq!(state.host_approval_forwarding.get(&host), Some(&true));
    assert!(
        state
            .host_resolvable_providers
            .get(&host)
            .expect("providers recovered")
            .contains("anthropic")
    );
    assert_eq!(
        state.host_live_endpoints.get(&host),
        Some(&LiveWsEndpointUrl("wss://hosts/host-b/live".to_string()))
    );
    check_multi_host_invariants(state);

    // A recovered host is a full bind fact: rebind at a higher epoch works
    // without any fresh ceremony.
    let rebound = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::HostRebound {
            host_id: host.clone(),
            epoch: 8,
            binding_generation: 1,
            protocol_min: 4,
            protocol_max: 4,
            engine_version: "0.7.23-test".to_string(),
            durable_sessions: true,
            autonomous_members: false,
            hard_cancel_member: true,
            tracked_input_cancel: true,
            memory_store: true,
            mcp: false,
            resolvable_providers: BTreeSet::new(),
            approval_forwarding: true,
            live_endpoint: None,
        },
    )
    .expect("rebind over a recovered binding accepted");
    assert!(rebound.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::HostReboundRecorded { epoch: 8, .. }
    )));

    // Double recovery for the same host fails loud on the guard instead of
    // silently overwriting live facts (the RecoverSupervisorAuthority
    // posture).
    assert!(
        authority.apply_signal(recover_signal(&host, true)).is_err(),
        "recovering an already-tracked host must be rejected"
    );

    // A live-incapable record recovers with NO live endpoint entry (absence
    // is the fact; no shadow boolean).
    let host_c = host_id("host-c");
    authority
        .apply_signal(recover_signal(&host_c, false))
        .expect("recovery of a second host must fire");
    assert!(
        !authority.state().host_live_endpoints.contains_key(&host_c),
        "live-incapable recovery must not synthesize a live endpoint"
    );
    check_multi_host_invariants(authority.state());
}

#[test]
fn recovered_generation_highwater_forces_fresh_bind_to_advance() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-highwater");
    authority
        .apply_signal(MobMachineSignal::RecoverHostBindingGenerationHighwater {
            host_id: host.clone(),
            binding_generation: 4,
        })
        .expect("durable generation tombstone recovers");
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::BeginHostBind {
                host_id: host.clone(),
                expected_endpoint: PeerAddress("tcp://hosts/highwater".to_string()),
                binding_generation: 4,
            },
        )
        .is_err(),
        "a fresh bind may not reuse the recovered generation tombstone",
    );
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginHostBind {
            host_id: host,
            expected_endpoint: PeerAddress("tcp://hosts/highwater".to_string()),
            binding_generation: 5,
        },
    )
    .expect("the next generation advances beyond recovered highwater");
}

#[test]
fn legacy_zero_generation_binding_recovers_revokes_and_reopens_at_one() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-legacy-zero");
    authority
        .apply_signal(MobMachineSignal::RecoverHostBinding {
            host_id: host.clone(),
            pubkey: PeerSigningKey([9; 32]),
            endpoint: PeerAddress("tcp://hosts/legacy-zero".to_string()),
            epoch: 7,
            binding_generation: 0,
            protocol_min: 4,
            protocol_max: 4,
            engine_version: "legacy".to_string(),
            durable_sessions: true,
            autonomous_members: false,
            hard_cancel_member: false,
            tracked_input_cancel: false,
            memory_store: false,
            mcp: false,
            resolvable_providers: BTreeSet::new(),
            approval_forwarding: false,
            live_endpoint: None,
        })
        .expect("legacy zero-generation active binding recovers");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host.clone(),
            binding_generation: 0,
        },
    )
    .expect("legacy binding revokes at its exact zero generation");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginHostBind {
            host_id: host,
            expected_endpoint: PeerAddress("tcp://hosts/legacy-zero".to_string()),
            binding_generation: 1,
        },
    )
    .expect("fresh binding starts at generation one after legacy zero");
}

// ==========================================================================
// Remote spawn ladder (§6.1/§15.4/§18.9/§19.L1/L5)
// ==========================================================================

/// Table-driven denial coverage: each observation fires exactly its typed
/// admission cause.
#[test]
fn remote_spawn_denials_are_typed_per_cause() {
    type Mutator = fn(&mut MobMachineInput);
    let flag_cases: [(&str, Mutator, MobSpawnMemberAdmissionKind); 10] = [
        (
            "rust_bundles",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    rust_bundles_present,
                    ..
                } = input
                {
                    *rust_bundles_present = true;
                }
            },
            MobSpawnMemberAdmissionKind::NonPortableRustBundles,
        ),
        (
            "per_spawn_external_tools",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    per_spawn_external_tools_present,
                    ..
                } = input
                {
                    *per_spawn_external_tools_present = true;
                }
            },
            MobSpawnMemberAdmissionKind::NonPortablePerSpawnExternalTools,
        ),
        (
            "mob_default_external_tools",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    mob_default_external_tools_present,
                    ..
                } = input
                {
                    *mob_default_external_tools_present = true;
                }
            },
            MobSpawnMemberAdmissionKind::NonPortableMobDefaultExternalTools,
        ),
        (
            "default_llm_client_override",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    default_llm_client_override_present,
                    ..
                } = input
                {
                    *default_llm_client_override_present = true;
                }
            },
            MobSpawnMemberAdmissionKind::NonPortableDefaultLlmClientOverride,
        ),
        (
            "host_surface_mcp_allowlist",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    host_surface_mcp_allowlist_present,
                    ..
                } = input
                {
                    *host_surface_mcp_allowlist_present = true;
                }
            },
            MobSpawnMemberAdmissionKind::NonPortableHostSurfaceMcpAllowlist,
        ),
        (
            "inherited_tool_filter",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    inherited_tool_filter_present,
                    ..
                } = input
                {
                    *inherited_tool_filter_present = true;
                }
            },
            MobSpawnMemberAdmissionKind::NonPortableInheritedToolFilter,
        ),
        (
            "workgraph_required",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    workgraph_required, ..
                } = input
                {
                    *workgraph_required = true;
                }
            },
            MobSpawnMemberAdmissionKind::NonPortableWorkgraphTools,
        ),
        (
            "shell_env",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    shell_env_present, ..
                } = input
                {
                    *shell_env_present = true;
                }
            },
            MobSpawnMemberAdmissionKind::SecretBearingShellEnv,
        ),
        (
            "mcp_stdio_env",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    mcp_stdio_env_present,
                    ..
                } = input
                {
                    *mcp_stdio_env_present = true;
                }
            },
            MobSpawnMemberAdmissionKind::SecretBearingMcpStdioEnv,
        ),
        (
            "mcp_http_headers",
            |input| {
                if let MobMachineInput::BeginSpawnExec {
                    mcp_http_headers_present,
                    ..
                } = input
                {
                    *mcp_http_headers_present = true;
                }
            },
            MobSpawnMemberAdmissionKind::SecretBearingMcpHttpHeaders,
        ),
    ];

    let host = host_id("host-b");
    for (label, mutate, expected) in flag_cases {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        bind_host(&mut authority, &host, 1, HostCaps::FULL);
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            0,
            Some(resolved_spec_digest("alpha", 0)),
        );
        let mut input = begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::TurnDriven);
        mutate(&mut input);
        let transition = MobMachineMutator::apply(&mut authority, input)
            .expect("denial arms are pure classification self-loops");
        assert_eq!(
            spawn_admission(&transition),
            Some(expected),
            "portability flag {label} must resolve its own typed denial",
        );
        assert!(
            !authority
                .state()
                .spawn_exec_phase
                .contains_key(&identity("alpha")),
            "a denied remote spawn must not open a ladder window ({label})",
        );
        assert!(
            !authority
                .state()
                .member_placement
                .contains_key(&identity("alpha")),
            "a denied remote spawn must not record placement ({label})",
        );
    }

    // Host not bound.
    {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            0,
            Some(resolved_spec_digest("alpha", 0)),
        );
        let transition = MobMachineMutator::apply(
            &mut authority,
            begin_spawn_remote(
                "alpha",
                0,
                &host_id("never-bound"),
                SpawnPolicyRuntimeMode::TurnDriven,
            ),
        )
        .expect("host-not-bound denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::HostNotBound),
        );
    }

    // Missing capability: autonomous_members for an AutonomousHost spawn.
    {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        bind_host(
            &mut authority,
            &host,
            1,
            HostCaps {
                autonomous_members: false,
                ..HostCaps::FULL
            },
        );
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            0,
            Some(resolved_spec_digest("alpha", 0)),
        );
        let transition = MobMachineMutator::apply(
            &mut authority,
            begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::AutonomousHost),
        )
        .expect("missing autonomous_members denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::MissingHostCapabilityAutonomousMembers),
        );
    }

    // Missing capability: durable_sessions for an AutonomousHost spawn.
    {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        bind_host(
            &mut authority,
            &host,
            1,
            HostCaps {
                durable_sessions: false,
                ..HostCaps::FULL
            },
        );
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            0,
            Some(resolved_spec_digest("alpha", 0)),
        );
        let transition = MobMachineMutator::apply(
            &mut authority,
            begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::AutonomousHost),
        )
        .expect("missing durable_sessions denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::MissingHostCapabilityDurableSessions),
        );
    }

    // Missing capability: tracked_input_cancel for an AutonomousHost spawn.
    {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        bind_host(
            &mut authority,
            &host,
            1,
            HostCaps {
                tracked_input_cancel: false,
                ..HostCaps::FULL
            },
        );
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            0,
            Some(resolved_spec_digest("alpha", 0)),
        );
        let transition = MobMachineMutator::apply(
            &mut authority,
            begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::AutonomousHost),
        )
        .expect("missing tracked_input_cancel denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::MissingHostCapabilityTrackedInputCancel),
        );
    }

    // Missing capability: the host's protocol range excludes tracked-turn V4.
    {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        bind_host(&mut authority, &host, 1, HostCaps::FULL);
        MobMachineMutator::apply(
            &mut authority,
            refresh_host_capabilities_input_with_tracked_contract(&host, 5, 5, true, true),
        )
        .expect("unplaced host may advertise a newer-only protocol range");
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            0,
            Some(resolved_spec_digest("alpha", 0)),
        );
        let transition = MobMachineMutator::apply(
            &mut authority,
            begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::AutonomousHost),
        )
        .expect("missing protocol V4 denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::MissingHostCapabilityProtocolV4),
        );
    }

    // Missing capability: memory_store when profiled.
    {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        bind_host(
            &mut authority,
            &host,
            1,
            HostCaps {
                memory_store: false,
                ..HostCaps::FULL
            },
        );
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            0,
            Some(resolved_spec_digest("alpha", 0)),
        );
        let mut input = begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::TurnDriven);
        if let MobMachineInput::BeginSpawnExec {
            memory_required, ..
        } = &mut input
        {
            *memory_required = true;
        }
        let transition = MobMachineMutator::apply(&mut authority, input)
            .expect("missing memory_store denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::MissingHostCapabilityMemoryStore),
        );
    }

    // Missing capability: mcp when profiled.
    {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        bind_host(
            &mut authority,
            &host,
            1,
            HostCaps {
                mcp: false,
                ..HostCaps::FULL
            },
        );
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            0,
            Some(resolved_spec_digest("alpha", 0)),
        );
        let mut input = begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::TurnDriven);
        if let MobMachineInput::BeginSpawnExec { mcp_required, .. } = &mut input {
            *mcp_required = true;
        }
        let transition =
            MobMachineMutator::apply(&mut authority, input).expect("missing mcp denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::MissingHostCapabilityMcp),
        );
    }

    // §19.L5: owner-bridge-less mobs cannot place members remotely.
    {
        let mut authority = MobMachineAuthority::new();
        bind_host(&mut authority, &host, 1, HostCaps::FULL);
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            0,
            Some(resolved_spec_digest("alpha", 0)),
        );
        let transition = MobMachineMutator::apply(
            &mut authority,
            begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::TurnDriven),
        )
        .expect("owner-bridge-absent denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::OwnerBridgeSessionAbsent),
        );
    }

    // §19.L1: a resume id bound to a different placement is a typed mismatch.
    {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        bind_host(&mut authority, &host, 1, HostCaps::FULL);
        let local_binding = seed_local_member(
            &mut authority,
            "alpha",
            0,
            SpawnPolicyRuntimeMode::TurnDriven,
        );
        authorize_spawn_profile(
            &mut authority,
            "alpha",
            1,
            Some(resolved_spec_digest("alpha", 1)),
        );
        let mut input = begin_spawn_remote("alpha", 1, &host, SpawnPolicyRuntimeMode::TurnDriven);
        if let MobMachineInput::BeginSpawnExec {
            resume_session_id, ..
        } = &mut input
        {
            *resume_session_id = Some(local_binding);
        }
        let transition = MobMachineMutator::apply(&mut authority, input)
            .expect("resume placement mismatch denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::LaunchModePlacementMismatch),
        );
    }

    // §15.4: remote placement requires the resolved spec digest.
    {
        let mut authority = MobMachineAuthority::new();
        bind_owner_bridge(&mut authority);
        bind_host(&mut authority, &host, 1, HostCaps::FULL);
        authorize_spawn_profile(&mut authority, "alpha", 0, None);
        let transition = MobMachineMutator::apply(
            &mut authority,
            begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::TurnDriven),
        )
        .expect("digest-absent denial fires");
        assert_eq!(
            spawn_admission(&transition),
            Some(MobSpawnMemberAdmissionKind::ResolvedSpecDigestAbsent),
        );
    }
}

#[test]
fn remote_spawn_ladder_enters_materialize_pending_and_commits_from_ack() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    let resolved = resolved_spec_digest("alpha", 0);
    authorize_spawn_profile(&mut authority, "alpha", 0, Some(resolved.clone()));

    let opened = MobMachineMutator::apply(
        &mut authority,
        begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::TurnDriven),
    )
    .expect("all-clean remote spawn must open MaterializePending");
    assert_eq!(
        authority.state().spawn_exec_phase.get(&identity("alpha")),
        Some(&SpawnExecPhase::MaterializePending),
    );
    assert_eq!(
        authority.state().member_placement.get(&identity("alpha")),
        Some(&host),
    );
    let request = opened.effects().iter().find_map(|effect| match effect {
        MobMachineEffect::RequestMemberMaterialization {
            agent_identity,
            spec_digest,
            host,
            ..
        } => Some((agent_identity.clone(), spec_digest.clone(), host.clone())),
        _ => None,
    });
    assert_eq!(
        request,
        Some((identity("alpha"), resolved.clone(), host.clone())),
        "the materialization request must carry the MACHINE-authorized digest",
    );

    // A mismatched digest echo must not commit (fail-closed).
    let ack_session = session_id("member-host-session-alpha");
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            commit_membership_remote(
                "alpha",
                0,
                &ack_session,
                Some("tampered-digest".to_string()),
                "0.7.22-test",
            ),
        )
        .is_err(),
        "a mismatched spec digest echo must reject the remote membership commit",
    );

    // A mismatched engine version must not commit either (the
    // HostEngineVersionChanged failure path applies instead).
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            commit_membership_remote("alpha", 0, &ack_session, Some(resolved.clone()), "9.9.9"),
        )
        .is_err(),
        "an ack engine version differing from the bound host record must reject the commit",
    );

    // The matched ack commits: roster truth published from the ack.
    let commit = MobMachineMutator::apply(
        &mut authority,
        commit_membership_remote("alpha", 0, &ack_session, Some(resolved), "0.7.22-test"),
    )
    .expect("matched ack must commit the remote membership");
    let state = authority.state();
    assert_eq!(
        state.spawn_exec_phase.get(&identity("alpha")),
        Some(&SpawnExecPhase::MembershipCommitted),
    );
    assert_eq!(
        state.member_session_bindings.get(&identity("alpha")),
        Some(&ack_session),
    );
    assert_eq!(
        state.member_peer_ids.get(&identity("alpha")),
        Some(&PeerId("peer-alpha".to_string())),
    );
    assert_eq!(
        state.member_peer_endpoints.get(&identity("alpha")),
        Some(&member_endpoint("alpha", 3)),
    );
    assert!(
        !state
            .spawn_profile_authority_resolved_spec_digests
            .contains_key(&identity("alpha")),
        "the single-shot resolved digest must be consumed at commit",
    );
    assert!(commit.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::MemberPeerRegistered { agent_identity, peer_id }
            if agent_identity == &identity("alpha") && peer_id == &PeerId("peer-alpha".to_string())
    )));
    assert!(commit.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::MemberSessionBindingChanged {
            old_session_id: None,
            new_session_id: Some(_),
            ..
        }
    )));
    check_multi_host_invariants(state);
}

#[test]
fn remote_materialize_ack_cannot_cross_revoke_and_fresh_host_binding() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-binding-aba");
    bind_owner_bridge(&mut authority);
    bind_host_at_generation(&mut authority, &host, 1, 1, HostCaps::FULL);
    let resolved = resolved_spec_digest("alpha", 0);
    authorize_spawn_profile(&mut authority, "alpha", 0, Some(resolved.clone()));
    MobMachineMutator::apply(
        &mut authority,
        begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::TurnDriven),
    )
    .expect("old binding opens remote materialization");
    assert_eq!(
        authority
            .state()
            .pending_placed_spawn_host_binding_generations
            .get(&identity("alpha")),
        Some(&1),
    );

    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host.clone(),
            binding_generation: 1,
        },
    )
    .expect("old binding revokes");
    bind_host_at_generation(&mut authority, &host, 1, 2, HostCaps::FULL);

    let ack_session = session_id("old-binding-materialize-ack");
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            commit_membership_remote("alpha", 0, &ack_session, Some(resolved), "0.7.22-test",),
        )
        .is_err(),
        "an ACK minted under binding generation 1 must not publish membership under generation 2",
    );
    assert!(
        !authority
            .state()
            .identity_to_runtime
            .contains_key(&identity("alpha"))
    );
    assert!(
        !authority
            .state()
            .member_session_bindings
            .contains_key(&identity("alpha"))
    );
}

#[test]
fn materialization_abort_retains_placement_until_exact_carrier_cleanup_resolves() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    authorize_spawn_profile(
        &mut authority,
        "alpha",
        0,
        Some(resolved_spec_digest("alpha", 0)),
    );
    MobMachineMutator::apply(
        &mut authority,
        begin_spawn_remote("alpha", 0, &host, SpawnPolicyRuntimeMode::TurnDriven),
    )
    .expect("remote spawn opens MaterializePending");

    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RecordMemberMaterializationFailure {
            agent_identity: identity("alpha"),
            kind: "HostEngineVersionChanged".to_string(),
        },
    )
    .expect("failure recording accepted while MaterializePending");
    assert_eq!(
        authority
            .state()
            .member_materialization_failures
            .get(&identity("alpha")),
        Some(&"HostEngineVersionChanged".to_string()),
    );
    assert_eq!(
        authority.state().spawn_exec_phase.get(&identity("alpha")),
        Some(&SpawnExecPhase::MaterializePending),
        "the ladder rung must stay MaterializePending so retry is idempotent",
    );

    // Final abort closes the spawn rung and clears the transient failure, but
    // retains placement and the exact pending-carrier tuple until the durable
    // compare-delete has been authorized and confirmed.
    let abort = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::AbortSpawnExec {
            agent_identity: identity("alpha"),
        },
    )
    .expect("abort accepted");
    let state = authority.state();
    assert!(!state.spawn_exec_phase.contains_key(&identity("alpha")));
    assert_eq!(state.member_placement.get(&identity("alpha")), Some(&host));
    assert!(
        !state
            .member_materialization_failures
            .contains_key(&identity("alpha"))
    );
    let obligation = state
        .pending_placed_carrier_cleanup
        .iter()
        .next()
        .cloned()
        .expect("abort must open exact pending-carrier cleanup authority");
    assert!(abort.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::PlacedCarrierCleanupRequested { obligation: emitted }
            if emitted == &obligation
    )));

    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::AuthorizePlacedCarrierCleanup {
            obligation: obligation.clone(),
        },
    )
    .expect("exact pending-carrier cleanup must be authorized");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::ResolvePlacedCarrierCleanup { obligation },
    )
    .expect("confirmed pending-carrier deletion must resolve cleanup");
    let state = authority.state();
    assert!(!state.member_placement.contains_key(&identity("alpha")));
    assert!(
        !state
            .pending_placed_spawn_ids
            .contains_key(&identity("alpha"))
    );
    assert!(state.pending_placed_carrier_cleanup.is_empty());
}

// ==========================================================================
// Flow step dispatch classification (§18.9)
// ==========================================================================

#[test]
fn classify_flow_step_dispatch_covers_the_four_outcome_matrix() {
    let mut authority = MobMachineAuthority::new();
    let durable_host = host_id("host-durable");
    let ephemeral_host = host_id("host-ephemeral");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &durable_host, 1, HostCaps::FULL);
    bind_host(
        &mut authority,
        &ephemeral_host,
        1,
        HostCaps {
            durable_sessions: false,
            ..HostCaps::FULL
        },
    );
    seed_local_member(
        &mut authority,
        "local-turn",
        0,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    seed_local_member(
        &mut authority,
        "local-auto",
        0,
        SpawnPolicyRuntimeMode::AutonomousHost,
    );
    seed_remote_member(
        &mut authority,
        "remote-turn",
        0,
        &durable_host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    seed_remote_member(
        &mut authority,
        "remote-auto",
        0,
        &durable_host,
        SpawnPolicyRuntimeMode::AutonomousHost,
    );
    seed_remote_member(
        &mut authority,
        "remote-ephemeral",
        0,
        &ephemeral_host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );

    let mut classify = |target: &str, overlay: bool| -> FlowStepDispatchKind {
        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ClassifyFlowStepDispatch {
                run_id: RunId("run-1".to_string()),
                step_id: StepId("step-1".to_string()),
                target: identity(target),
                overlay_present: overlay,
            },
        )
        .expect("dispatch classification is total over roster members");
        transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                MobMachineEffect::FlowStepDispatchClassified { dispatch, .. } => Some(*dispatch),
                _ => None,
            })
            .expect("classification must emit a dispatch verdict")
    };

    assert_eq!(classify("local-turn", false), FlowStepDispatchKind::Local);
    assert_eq!(classify("local-turn", true), FlowStepDispatchKind::Local);
    assert_eq!(
        classify("remote-turn", false),
        FlowStepDispatchKind::RemoteTurnDirective,
    );
    assert_eq!(
        classify("remote-turn", true),
        FlowStepDispatchKind::RemoteTurnDirective,
    );
    // Overlay + autonomous rejects REGARDLESS of placement.
    assert_eq!(
        classify("local-auto", true),
        FlowStepDispatchKind::RejectedOverlayAutonomous,
    );
    assert_eq!(
        classify("remote-auto", true),
        FlowStepDispatchKind::RejectedOverlayAutonomous,
    );
    assert_eq!(
        classify("remote-auto", false),
        FlowStepDispatchKind::RemoteTurnDirective,
    );
    // durable_sessions = false host: tracked turns need the durable journal.
    assert_eq!(
        classify("remote-ephemeral", false),
        FlowStepDispatchKind::RejectedHostIncapable,
    );
}

// ==========================================================================
// Obligation windows (§6.2 D4, §18 O2)
// ==========================================================================

#[test]
fn remote_turn_obligations_are_idempotent_set_semantics() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    seed_remote_member(
        &mut authority,
        "remote-alpha",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );

    let obligation = RemoteTurnObligation {
        agent_identity: identity("remote-alpha"),
        host_id: host.clone(),
        host_binding_generation: 1,
        member_session_id: session_id("member-host-session-remote-alpha"),
        generation: Generation(0),
        fence_token: FenceToken(1),
        dispatch_sequence: 1,
        input_id: InputId("input-1".to_string()),
        run_id: RunId("run-1".to_string()),
        step_id: StepId("step-1".to_string()),
    };

    for _ in 0..2 {
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordRemoteTurnObligation {
                obligation: obligation.clone(),
            },
        )
        .expect("recording is idempotent");
        assert!(
            authority
                .state()
                .pending_remote_turn_outcomes
                .contains(&obligation),
        );
    }
    seed_remote_member(
        &mut authority,
        "remote-beta",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    let cross_member_same_input = RemoteTurnObligation {
        agent_identity: identity("remote-beta"),
        host_id: host.clone(),
        host_binding_generation: 1,
        member_session_id: session_id("member-host-session-remote-beta"),
        generation: Generation(0),
        fence_token: FenceToken(1),
        dispatch_sequence: 2,
        input_id: obligation.input_id.clone(),
        run_id: RunId("run-cross-member".to_string()),
        step_id: StepId("step-cross-member".to_string()),
    };
    for _ in 0..2 {
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::CommitRemoteTurnOutcome {
                obligation: obligation.clone(),
            },
        )
        .expect("commit is idempotent");
        assert!(
            authority
                .state()
                .committed_remote_turn_outcomes
                .contains(&obligation),
        );
    }
    for _ in 0..2 {
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveRemoteTurnObligation {
                obligation: obligation.clone(),
            },
        )
        .expect("resolution is idempotent");
        assert!(
            authority
                .state()
                .resolved_remote_turn_outcomes
                .contains(&obligation),
        );
    }
    for _ in 0..2 {
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::AcknowledgeRemoteTurnOutcome {
                obligation: obligation.clone(),
            },
        )
        .expect("ACK confirmation is idempotent");
    }
    assert!(authority.state().resolved_remote_turn_outcomes.is_empty());
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RecordRemoteTurnObligation {
            obligation: obligation.clone(),
        },
    )
    .expect("delayed Record after ACK is an historical replay");
    assert!(
        authority.state().pending_remote_turn_outcomes.is_empty(),
        "dispatch high-watermark must prevent delayed Record resurrection"
    );

    for (label, stale) in [
        (
            "host binding generation",
            RemoteTurnObligation {
                host_binding_generation: 2,
                dispatch_sequence: 2,
                input_id: InputId("stale-host-binding-generation-input".to_string()),
                ..obligation.clone()
            },
        ),
        (
            "session",
            RemoteTurnObligation {
                member_session_id: session_id("stale-session"),
                dispatch_sequence: 2,
                input_id: InputId("stale-session-input".to_string()),
                ..obligation.clone()
            },
        ),
        (
            "generation",
            RemoteTurnObligation {
                generation: Generation(1),
                dispatch_sequence: 2,
                input_id: InputId("stale-generation-input".to_string()),
                ..obligation.clone()
            },
        ),
        (
            "fence",
            RemoteTurnObligation {
                fence_token: FenceToken(2),
                dispatch_sequence: 2,
                input_id: InputId("stale-fence-input".to_string()),
                ..obligation
            },
        ),
    ] {
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::RecordRemoteTurnObligation { obligation: stale },
            )
            .is_err(),
            "a stale placed-member {label} tuple must be rejected before custody opens",
        );
    }

    // The obligation target must be a placed member.
    let unplaced = RemoteTurnObligation {
        agent_identity: identity("nobody"),
        host_id: host.clone(),
        host_binding_generation: 1,
        member_session_id: session_id("member-host-session-nobody"),
        generation: Generation(0),
        fence_token: FenceToken(1),
        dispatch_sequence: 2,
        input_id: InputId("input-2".to_string()),
        run_id: RunId("run-2".to_string()),
        step_id: StepId("step-2".to_string()),
    };
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordRemoteTurnObligation {
                obligation: unplaced
            },
        )
        .is_err(),
        "an obligation for an unplaced identity must be rejected",
    );

    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RecordRemoteTurnObligation {
            obligation: cross_member_same_input.clone(),
        },
    )
    .expect("the exact custody key scopes an opaque input id to its member residency");
    assert!(
        authority
            .state()
            .pending_remote_turn_outcomes
            .contains(&cross_member_same_input),
        "equal input ids on distinct members name distinct custody rows",
    );
}

/// U-3 extension (phase 6): dispatch classification is total over ROSTER
/// members only — a non-roster target fires no admitting arm (typed machine
/// reject, never a phantom verdict).
#[test]
fn classify_flow_step_dispatch_rejects_non_roster_target() {
    let mut authority = MobMachineAuthority::new();
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host_id("host-durable"), 1, HostCaps::FULL);
    seed_local_member(
        &mut authority,
        "rostered",
        0,
        SpawnPolicyRuntimeMode::TurnDriven,
    );

    // A placement-less UNKNOWN identity still classifies Local (placement
    // absent) — classification is a placement fact read, and the runtime
    // resolves roster existence separately. The BOUNDARY this pins: a
    // completely absent identity has no runtime mode, so an overlay-bearing
    // classification of it cannot fire the overlay-autonomous arm.
    let transition = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::ClassifyFlowStepDispatch {
            run_id: RunId("run-x".to_string()),
            step_id: StepId("step-x".to_string()),
            target: identity("never-spawned"),
            overlay_present: true,
        },
    )
    .expect("classification stays total (placement facts only)");
    let dispatch = transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            MobMachineEffect::FlowStepDispatchClassified { dispatch, .. } => Some(*dispatch),
            _ => None,
        })
        .expect("a dispatch verdict is emitted");
    assert_eq!(
        dispatch,
        FlowStepDispatchKind::Local,
        "an unknown identity classifies Local; the executor's roster lookup \
         is the existence gate (MemberNotFound), never the machine arm",
    );
}

/// U-3 extension (phase 6): obligation-set interplay — recording guards on
/// `target_placed`; resolution is per_phase over Running/Stopped/Completed/
/// Destroyed and stays set-idempotent after lifecycle transitions.
#[test]
fn remote_turn_obligation_resolution_survives_lifecycle_phases() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    seed_remote_member(
        &mut authority,
        "remote-phase",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );

    let obligation = RemoteTurnObligation {
        agent_identity: identity("remote-phase"),
        host_id: host.clone(),
        host_binding_generation: 1,
        member_session_id: session_id("member-host-session-remote-phase"),
        generation: Generation(0),
        fence_token: FenceToken(1),
        dispatch_sequence: 1,
        input_id: InputId("phase-input".to_string()),
        run_id: RunId("phase-run".to_string()),
        step_id: StepId("phase-step".to_string()),
    };
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RecordRemoteTurnObligation {
            obligation: obligation.clone(),
        },
    )
    .expect("obligation records while Running");
    // Stop with Pending custody, then drive every post-effect custody phase.
    // `per_phase` must rewrite the generated target to Stopped rather than
    // laundering cleanup into a lifecycle resurrection.
    begin_lifecycle_quiesce(&mut authority, PlacedCompletionLifecycleIntentKind::Stop);
    MobMachineMutator::apply(&mut authority, MobMachineInput::Stop)
        .expect("mob stops with an outstanding obligation");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::CommitRemoteTurnOutcome {
            obligation: obligation.clone(),
        },
    )
    .expect("durable step terminal commits custody in Stopped");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Stopped);
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::ResolveRemoteTurnObligation {
            obligation: obligation.clone(),
        },
    )
    .expect("resolution fires in Stopped");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Stopped);
    assert!(
        !authority
            .state()
            .committed_remote_turn_outcomes
            .contains(&obligation),
    );
    // Idempotent replay in the same phase.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::ResolveRemoteTurnObligation {
            obligation: obligation.clone(),
        },
    )
    .expect("resolution replay is a set-idempotent no-op in Stopped");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Stopped);
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::AcknowledgeRemoteTurnOutcome {
            obligation: obligation.clone(),
        },
    )
    .expect("ACK closes custody in Stopped");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Stopped);
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::DisposeRemoteTurnObligation { obligation },
    )
    .expect("release disposal replay is accepted in Stopped");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Stopped);
}

#[test]
fn remote_turn_post_destroy_cleanup_preserves_destroyed_phase() {
    let mut authority = MobMachineAuthority::new();
    begin_lifecycle_quiesce(&mut authority, PlacedCompletionLifecycleIntentKind::Destroy);
    MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy)
        .expect("empty authority can destroy");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Destroyed);

    let obligation = RemoteTurnObligation::default();
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::AbortRemoteTurnObligation {
            obligation: obligation.clone(),
        },
    )
    .expect("post-destroy abort replay is accepted");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Destroyed);
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::CommitRemoteTurnOutcome {
            obligation: obligation.clone(),
        },
    )
    .expect("post-destroy commit replay is accepted");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Destroyed);
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::ResolveRemoteTurnObligation {
            obligation: obligation.clone(),
        },
    )
    .expect("post-destroy resolve replay is accepted");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Destroyed);
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::DisposeRemoteTurnObligation {
            obligation: obligation.clone(),
        },
    )
    .expect("post-destroy disposal replay is accepted");
    assert_eq!(authority.state().lifecycle_phase, MobPhase::Destroyed);
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::AcknowledgeRemoteTurnOutcome { obligation },
    )
    .expect("post-destroy ACK replay is accepted");
    assert_eq!(
        authority.state().lifecycle_phase,
        MobPhase::Destroyed,
        "per_phase cleanup must never resurrect lifecycle"
    );
}

#[test]
fn remote_turn_post_complete_cleanup_preserves_completed_phase() {
    let mut authority = MobMachineAuthority::new();
    begin_lifecycle_quiesce(
        &mut authority,
        PlacedCompletionLifecycleIntentKind::Complete,
    );
    MobMachineMutator::apply(&mut authority, MobMachineInput::Complete)
        .expect("empty authority can complete");
    let obligation = RemoteTurnObligation::default();
    for input in [
        MobMachineInput::AbortRemoteTurnObligation {
            obligation: obligation.clone(),
        },
        MobMachineInput::CommitRemoteTurnOutcome {
            obligation: obligation.clone(),
        },
        MobMachineInput::ResolveRemoteTurnObligation {
            obligation: obligation.clone(),
        },
        MobMachineInput::AcknowledgeRemoteTurnOutcome {
            obligation: obligation.clone(),
        },
        MobMachineInput::DisposeRemoteTurnObligation { obligation },
    ] {
        MobMachineMutator::apply(&mut authority, input)
            .expect("absent custody replay is accepted after completion");
        assert_eq!(
            authority.state().lifecycle_phase,
            MobPhase::Completed,
            "remote custody cleanup must preserve Completed"
        );
    }
}

#[test]
fn recovered_flow_cleanup_signals_preserve_every_lifecycle_phase() {
    fn apply_cleanup(authority: &mut MobMachineAuthority, expected: MobPhase) {
        authority
            .apply_signal(MobMachineSignal::CompleteFlow)
            .expect("CompleteFlow is lifecycle-total during recovery cleanup");
        assert_eq!(authority.state().lifecycle_phase, expected);
        authority
            .apply_signal(MobMachineSignal::FinishRun)
            .expect("FinishRun is lifecycle-total during recovery cleanup");
        assert_eq!(authority.state().lifecycle_phase, expected);
    }

    let mut running = MobMachineAuthority::new();
    apply_cleanup(&mut running, MobPhase::Running);

    let mut stopped = MobMachineAuthority::new();
    begin_lifecycle_quiesce(&mut stopped, PlacedCompletionLifecycleIntentKind::Stop);
    MobMachineMutator::apply(&mut stopped, MobMachineInput::Stop).expect("stop");
    apply_cleanup(&mut stopped, MobPhase::Stopped);

    let mut completed = MobMachineAuthority::new();
    begin_lifecycle_quiesce(
        &mut completed,
        PlacedCompletionLifecycleIntentKind::Complete,
    );
    MobMachineMutator::apply(&mut completed, MobMachineInput::Complete).expect("complete");
    apply_cleanup(&mut completed, MobPhase::Completed);

    let mut destroy_admitted = MobMachineAuthority::new();
    begin_lifecycle_quiesce(
        &mut destroy_admitted,
        PlacedCompletionLifecycleIntentKind::Destroy,
    );
    destroy_admitted
        .apply_signal(MobMachineSignal::AdmitDestroyCleanup)
        .expect("admit destroy cleanup");
    apply_cleanup(&mut destroy_admitted, MobPhase::Running);
    assert!(
        destroy_admitted.state().destroy_admitted,
        "flow cleanup cannot clear the admitted-destroy fence"
    );

    let mut storage_finalizing = MobMachineAuthority::new();
    begin_lifecycle_quiesce(
        &mut storage_finalizing,
        PlacedCompletionLifecycleIntentKind::Destroy,
    );
    storage_finalizing
        .apply_signal(MobMachineSignal::AdmitDestroyCleanup)
        .expect("admit destroy cleanup");
    MobMachineMutator::apply(&mut storage_finalizing, MobMachineInput::Destroy)
        .expect("destroy empty authority");
    storage_finalizing
        .apply_signal(MobMachineSignal::AdmitDestroyStorageFinalizing)
        .expect("admit storage finalizing");
    apply_cleanup(&mut storage_finalizing, MobPhase::Destroyed);
    assert!(storage_finalizing.state().destroy_admitted);
}

#[test]
fn recovered_custody_for_absent_target_fails_machine_invariant() {
    let authority = MobMachineAuthority::new();
    let mut state = authority.state().clone();
    state
        .pending_remote_turn_outcomes
        .insert(RemoteTurnObligation {
            agent_identity: identity("absent"),
            host_id: host_id("absent-host"),
            host_binding_generation: 9,
            member_session_id: session_id("absent-session"),
            generation: Generation(9),
            fence_token: FenceToken(11),
            dispatch_sequence: 1,
            input_id: InputId("orphan-input".to_string()),
            run_id: RunId("orphan-run".to_string()),
            step_id: StepId("orphan-step".to_string()),
        });
    assert!(
        MobMachineAuthority::recover_from_state(state).is_err(),
        "custody cannot survive without exact current placed target authority"
    );
}

#[test]
fn recovered_remote_custody_must_match_current_host_binding_generation() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    let member = identity("remote-alpha");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    seed_remote_member(
        &mut authority,
        "remote-alpha",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );

    let old_obligation = RemoteTurnObligation {
        agent_identity: member.clone(),
        host_id: host.clone(),
        host_binding_generation: 1,
        member_session_id: session_id("member-host-session-remote-alpha"),
        generation: Generation(0),
        fence_token: FenceToken(1),
        dispatch_sequence: 1,
        input_id: InputId("input-1".to_string()),
        run_id: RunId("run-1".to_string()),
        step_id: StepId("step-1".to_string()),
    };
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RecordRemoteTurnObligation {
            obligation: old_obligation.clone(),
        },
    )
    .expect("record generation-1 custody");

    let mut promoted = authority.state().clone();
    promoted
        .current_placed_spawn_host_binding_generations
        .insert(member, 2);
    promoted.host_binding_generations.insert(host.clone(), 2);
    promoted.host_binding_generation_highwater.insert(host, 2);

    let mut exact = promoted.clone();
    assert!(exact.pending_remote_turn_outcomes.remove(&old_obligation));
    exact
        .pending_remote_turn_outcomes
        .insert(RemoteTurnObligation {
            host_binding_generation: 2,
            ..old_obligation
        });
    MobMachineAuthority::recover_from_state(exact)
        .expect("custody matching the current placed carrier generation must recover");

    assert!(
        MobMachineAuthority::recover_from_state(promoted).is_err(),
        "generation-1 custody cannot survive after the placed carrier advances to generation 2",
    );
}

// ==========================================================================
// U-2 (phase 6) — host-side turn-outcome journal: Fresh/Replay dedup,
// exact residency scoping, release-time pruning (MobHostBindingAuthority)
// ==========================================================================

mod turn_outcome_journal {
    use meerkat_mob::machines::mob_host_binding_authority::{
        AgentIdentity as HostAgentIdentity, FenceToken as HostFenceToken, FlowTurnOutcomeKind,
        Generation as HostGeneration, InputId as HostInputId, MemberKey,
        MobHostBindingAuthorityAuthority, MobHostBindingAuthorityEffect,
        MobHostBindingAuthorityInput, MobHostBindingAuthorityMutator, MobId as HostMobId,
        PeerId as HostPeerId, PeerSigningKey as HostPeerSigningKey, SessionId as HostSessionId,
        TurnKey,
    };

    fn bound_authority(mob: &str) -> MobHostBindingAuthorityAuthority {
        let mut authority = MobHostBindingAuthorityAuthority::new();
        MobHostBindingAuthorityMutator::apply(
            &mut authority,
            MobHostBindingAuthorityInput::ResolveHostBind {
                mob_id: HostMobId(mob.to_string()),
                supervisor_peer_id: HostPeerId("supervisor-a".to_string()),
                supervisor_signing_key: HostPeerSigningKey([1; 32]),
                epoch: 1,
                binding_generation: 1,
                sender_matches_supervisor: true,
                address_matches: true,
                token_valid: true,
            },
        )
        .expect("host binds for the journal rows");
        authority
    }

    fn turn_key(mob: &str, identity: &str, generation: u64, input: &str) -> TurnKey {
        TurnKey {
            mob_id: HostMobId(mob.to_string()),
            agent_identity: HostAgentIdentity(identity.to_string()),
            generation: HostGeneration(generation),
            fence_token: HostFenceToken(generation),
            input_id: HostInputId(input.to_string()),
        }
    }

    fn materialize(
        authority: &mut MobHostBindingAuthorityAuthority,
        mob: &str,
        identity: &str,
        generation: u64,
    ) {
        MobHostBindingAuthorityMutator::apply(
            authority,
            MobHostBindingAuthorityInput::RecordMaterializedMember {
                member_key: MemberKey {
                    mob_id: HostMobId(mob.to_string()),
                    agent_identity: HostAgentIdentity(identity.to_string()),
                },
                generation: HostGeneration(generation),
                fence_token: HostFenceToken(generation),
                session_id: HostSessionId(format!("session-{identity}-{generation}")),
                spec_digest: format!("digest-{identity}-{generation}"),
            },
        )
        .expect("materialized row records");
    }

    fn reserve(authority: &mut MobHostBindingAuthorityAuthority, turn_key: &TurnKey) {
        MobHostBindingAuthorityMutator::apply(
            authority,
            MobHostBindingAuthorityInput::ReserveTurnOutcomePending {
                turn_key: turn_key.clone(),
                window_start: 1,
            },
        )
        .expect("Pending reservation records");
    }

    #[test]
    fn journal_dedup_replay_echoes_recorded_row() {
        let mut authority = bound_authority("mob-j1");
        materialize(&mut authority, "mob-j1", "worker-1", 1);
        let key = turn_key("mob-j1", "worker-1", 1, "input-1");
        reserve(&mut authority, &key);

        let fresh = MobHostBindingAuthorityMutator::apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: key.clone(),
                terminal_seq: 41,
                outcome: FlowTurnOutcomeKind::Completed,
            },
        )
        .expect("fresh journal row records");
        assert!(matches!(
            fresh.effects().first(),
            Some(MobHostBindingAuthorityEffect::TurnOutcomeRecorded {
                terminal_seq: 41,
                ..
            })
        ));

        // Byte-different replay at the SAME key: the RECORDED row wins and
        // is echoed verbatim — redelivery converges, never overwrites.
        let replay = MobHostBindingAuthorityMutator::apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: key.clone(),
                terminal_seq: 999,
                outcome: FlowTurnOutcomeKind::Failed,
            },
        )
        .expect("replay fires the dedup arm");
        match replay.effects().first() {
            Some(MobHostBindingAuthorityEffect::TurnOutcomeReplayed {
                terminal_seq,
                outcome,
                ..
            }) => {
                assert_eq!(*terminal_seq, 41, "replay echoes the recorded seq");
                assert_eq!(
                    *outcome,
                    FlowTurnOutcomeKind::Completed,
                    "replay echoes the recorded outcome kind"
                );
            }
            other => panic!("expected TurnOutcomeReplayed, got {other:?}"),
        }
        assert_eq!(
            authority.state().turn_outcome_terminal_seqs.get(&key),
            Some(&41),
            "the recorded row is untouched by the replay"
        );
    }

    #[test]
    fn superseding_materialization_prunes_previous_generation() {
        let mut authority = bound_authority("mob-j2");
        let gen1 = turn_key("mob-j2", "worker-1", 1, "input-1");
        let gen2 = turn_key("mob-j2", "worker-1", 2, "input-1");

        materialize(&mut authority, "mob-j2", "worker-1", 1);
        reserve(&mut authority, &gen1);
        MobHostBindingAuthorityMutator::apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: gen1.clone(),
                terminal_seq: 10,
                outcome: FlowTurnOutcomeKind::Completed,
            },
        )
        .expect("generation-one row records");

        materialize(&mut authority, "mob-j2", "worker-1", 2);
        assert!(
            !authority
                .state()
                .turn_outcome_terminal_seqs
                .contains_key(&gen1),
            "superseding materialization retires the previous residency's row"
        );

        let stale = MobHostBindingAuthorityMutator::apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: gen1,
                terminal_seq: 11,
                outcome: FlowTurnOutcomeKind::Failed,
            },
        )
        .expect("late watcher is classified explicitly");
        assert!(matches!(
            stale.effects().first(),
            Some(MobHostBindingAuthorityEffect::TurnOutcomeStaleDropped { .. })
        ));

        reserve(&mut authority, &gen2);
        MobHostBindingAuthorityMutator::apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordTurnOutcome {
                turn_key: gen2.clone(),
                terminal_seq: 3,
                outcome: FlowTurnOutcomeKind::Completed,
            },
        )
        .expect("current generation row records");
        assert_eq!(authority.state().turn_outcome_terminal_seqs.len(), 1);
        assert_eq!(
            authority.state().turn_outcome_terminal_seqs.get(&gen2),
            Some(&3)
        );
    }

    #[test]
    fn release_prunes_all_released_member_rows_but_keeps_other_members() {
        let mut authority = bound_authority("mob-j3");
        let member = MemberKey {
            mob_id: HostMobId("mob-j3".to_string()),
            agent_identity: HostAgentIdentity("worker-1".to_string()),
        };
        materialize(&mut authority, "mob-j3", "worker-1", 2);
        materialize(&mut authority, "mob-j3", "worker-2", 2);

        for (identity, input) in [
            ("worker-1", "released-member"),
            ("worker-2", "other-member"),
        ] {
            let key = turn_key("mob-j3", identity, 2, input);
            reserve(&mut authority, &key);
            MobHostBindingAuthorityMutator::apply(
                &mut authority,
                MobHostBindingAuthorityInput::RecordTurnOutcome {
                    turn_key: key,
                    terminal_seq: 7,
                    outcome: FlowTurnOutcomeKind::Completed,
                },
            )
            .expect("journal row records");
        }
        assert_eq!(authority.state().turn_outcome_terminal_seqs.len(), 2);

        // Release generation 2 of worker-1: no row for that identity may
        // remain once its only materialized owner is gone.
        MobHostBindingAuthorityMutator::apply(
            &mut authority,
            MobHostBindingAuthorityInput::RecordMemberRelease {
                member_key: member,
                generation: HostGeneration(2),
                fence_token: HostFenceToken(2),
                disposal:
                    meerkat_mob::machines::mob_host_binding_authority::MemberSessionDisposal::Archived,
            },
        )
        .expect("release records");

        let remaining: Vec<TurnKey> = authority
            .state()
            .turn_outcome_terminal_seqs
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            remaining.len(),
            1,
            "every worker-1 row is pruned: {remaining:?}"
        );
        assert!(
            !remaining
                .iter()
                .any(|key| key.agent_identity.0 == "worker-1"),
            "the released member's rows are gone"
        );
        assert!(
            remaining
                .iter()
                .any(|key| key.agent_identity.0 == "worker-2"),
            "other members' rows survive the release"
        );
    }
}

#[test]
fn route_install_obligations_validate_kind_against_wiring_graph() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);

    let edge = WiringEdge::new(identity("alpha"), identity("beta"));
    let install = RouteInstallObligation {
        edge: edge.clone(),
        host: host.clone(),
        kind: RouteObligationKind::Install,
    };

    // Install obligations require the edge to be wired.
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordRouteInstall {
                obligation: install.clone(),
            },
        )
        .is_err(),
        "an install obligation for an unwired edge must be rejected",
    );
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::WireMembers { edge: edge.clone() },
    )
    .expect("wire accepted");
    let recorded = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RecordRouteInstall {
            obligation: install.clone(),
        },
    )
    .expect("install obligation accepted once the edge is wired");
    assert!(recorded.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::RouteInstallRequested { obligation } if obligation == &install
    )));
    assert!(authority.state().pending_route_installs.contains(&install));

    // Resolve and rollback are idempotent removals.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::ResolveRouteInstall {
            obligation: install.clone(),
        },
    )
    .expect("resolve accepted");
    assert!(!authority.state().pending_route_installs.contains(&install));
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RollbackRouteInstall {
            obligation: install,
        },
    )
    .expect("rollback of an absent obligation is an idempotent no-op");

    // Remove is never admitted into the pending ledger, whether the edge is
    // still wired or already absent. It rides the separate synchronous
    // AuthorizeRouteRemovalBeforeUnwire input.
    let removal = RouteInstallObligation {
        edge: edge.clone(),
        host: host.clone(),
        kind: RouteObligationKind::Remove,
    };
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordRouteInstall {
                obligation: removal.clone(),
            },
        )
        .is_err(),
        "a removal obligation for a still-wired edge must be rejected",
    );
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveRouteInstall {
                obligation: removal.clone(),
            },
        )
        .is_err(),
        "ResolveRouteInstall must reject compatibility-only Remove values",
    );
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RollbackRouteInstall {
                obligation: removal.clone(),
            },
        )
        .is_err(),
        "RollbackRouteInstall must reject compatibility-only Remove values",
    );
    MobMachineMutator::apply(&mut authority, MobMachineInput::UnwireMembers { edge })
        .expect("unwire accepted");
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordRouteInstall {
                obligation: removal.clone(),
            },
        )
        .is_err(),
        "a removal must remain inadmissible after the edge is unwired",
    );
    assert!(!authority.state().pending_route_installs.contains(&removal));
    assert!(
        authority
            .state()
            .pending_route_installs
            .iter()
            .all(|obligation| obligation.kind == RouteObligationKind::Install)
    );
}

#[test]
fn route_removal_before_unwire_is_ephemeral_authority_not_pending_ledger() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    seed_remote_member(
        &mut authority,
        "alpha",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    seed_local_member(
        &mut authority,
        "beta",
        0,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RegisterMemberPeer {
            agent_identity: identity("beta"),
            agent_runtime_id: runtime_id("beta", 0),
            generation: Generation(0),
            fence_token: FenceToken(1),
            peer_endpoint: member_endpoint("beta", 5),
        },
    )
    .expect("local endpoint published");
    let edge = WiringEdge::new(identity("alpha"), identity("beta"));
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::WireMembers { edge: edge.clone() },
    )
    .expect("wire accepted");
    let removal = RouteInstallObligation {
        edge: edge.clone(),
        host,
        kind: RouteObligationKind::Remove,
    };
    let authorized = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::AuthorizeRouteRemovalBeforeUnwire {
            obligation: removal.clone(),
        },
    )
    .expect("exact synchronous removal authorized while edge is wired");
    assert!(authorized.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::RouteInstallRequested { obligation } if obligation == &removal
    )));
    assert!(authority.state().wiring_edges.contains(&edge));
    assert!(authority.state().pending_route_installs.is_empty());
}

/// Phase 4 — the `host_bound` guard on `RecordRouteInstall{Install}`
/// has its own pin: an obligation naming an UNBOUND host is rejected, a
/// half-bound (`Requested`) host is still rejected, and only a committed
/// bind admits the obligation and emits `RouteInstallRequested`.
#[test]
fn route_install_requires_bound_host() {
    let mut authority = MobMachineAuthority::new();
    bind_owner_bridge(&mut authority);
    let host = host_id("host-c");
    let edge = WiringEdge::new(identity("alpha"), identity("beta"));
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::WireMembers { edge: edge.clone() },
    )
    .expect("wire accepted");
    let install = RouteInstallObligation {
        edge,
        host: host.clone(),
        kind: RouteObligationKind::Install,
    };

    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordRouteInstall {
                obligation: install.clone(),
            },
        )
        .is_err(),
        "an obligation naming an unbound host must be rejected",
    );

    // A Requested (half-bound) host is NOT Bound — still rejected.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::BeginHostBind {
            host_id: host.clone(),
            expected_endpoint: PeerAddress(format!("tcp://hosts/{}", host.0)),
            binding_generation: 1,
        },
    )
    .expect("BeginHostBind must open the bind window");
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordRouteInstall {
                obligation: install.clone(),
            },
        )
        .is_err(),
        "a half-bound (Requested) host must be rejected",
    );

    MobMachineMutator::apply(
        &mut authority,
        commit_host_bind_input(&host, 1, HostCaps::FULL),
    )
    .expect("CommitHostBind must commit the requested bind");
    let recorded = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RecordRouteInstall {
            obligation: install.clone(),
        },
    )
    .expect("install obligation accepted once the host is Bound");
    assert!(
        recorded.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::RouteInstallRequested { obligation } if obligation == &install
        )),
        "the admitted obligation emits RouteInstallRequested",
    );
    assert!(authority.state().pending_route_installs.contains(&install));
}

// ==========================================================================
// Grants (§6.5/§8)
// ==========================================================================

#[test]
fn grant_lifecycle_stores_expiry_as_data_and_revalidates_revoke_partitions() {
    let mut authority = MobMachineAuthority::new();
    let principal = PrincipalId("console:luka".to_string());
    let scopes = BTreeSet::from([
        ControlScope::List,
        ControlScope::ReadHistory,
        ControlScope::Cancel,
    ]);

    let granted = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::GrantOperatorScopes {
            principal: principal.clone(),
            scopes: scopes.clone(),
            expires_at_ms: Some(1_234_567),
        },
    )
    .expect("grant accepted");
    assert!(granted.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::GrantRecorded {
            expires_at_ms: Some(1_234_567),
            ..
        }
    )));
    // Expiry is DATA: the machine stores the raw timestamp and no grant
    // input carries a clock reading to compare it against.
    assert_eq!(
        authority.state().operator_grant_scopes.get(&principal),
        Some(&scopes),
    );
    assert_eq!(
        authority.state().operator_grant_expiries.get(&principal),
        Some(&Some(1_234_567)),
    );

    // A partition that does not cover the recorded grant is rejected.
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RevokeOperatorScopes {
                principal: principal.clone(),
                revoked: BTreeSet::from([ControlScope::Cancel]),
                remaining: BTreeSet::from([ControlScope::List]),
            },
        )
        .is_err(),
        "a proposed partition omitting a granted scope must be rejected",
    );
    // A partition claiming a never-granted scope is rejected.
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RevokeOperatorScopes {
                principal: principal.clone(),
                revoked: BTreeSet::from([ControlScope::AdminHost]),
                remaining: BTreeSet::from([
                    ControlScope::List,
                    ControlScope::ReadHistory,
                    ControlScope::Cancel
                ]),
            },
        )
        .is_err(),
        "revoking a scope that was never granted must be rejected",
    );

    // Partial revoke replaces the scope set and keeps the expiry.
    let partial = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeOperatorScopes {
            principal: principal.clone(),
            revoked: BTreeSet::from([ControlScope::Cancel]),
            remaining: BTreeSet::from([ControlScope::List, ControlScope::ReadHistory]),
        },
    )
    .expect("validated partial revoke accepted");
    assert!(
        partial
            .effects()
            .iter()
            .any(|effect| matches!(effect, MobMachineEffect::GrantRevoked { .. }))
    );
    assert_eq!(
        authority.state().operator_grant_scopes.get(&principal),
        Some(&BTreeSet::from([
            ControlScope::List,
            ControlScope::ReadHistory,
        ])),
    );
    assert_eq!(
        authority.state().operator_grant_expiries.get(&principal),
        Some(&Some(1_234_567)),
        "partial revoke retains the recorded expiry",
    );

    // Full revoke removes both maps.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeOperatorScopes {
            principal: principal.clone(),
            revoked: BTreeSet::from([ControlScope::List, ControlScope::ReadHistory]),
            remaining: BTreeSet::new(),
        },
    )
    .expect("full revoke accepted");
    assert!(
        !authority
            .state()
            .operator_grant_scopes
            .contains_key(&principal)
    );
    assert!(
        !authority
            .state()
            .operator_grant_expiries
            .contains_key(&principal)
    );

    // Revoking an absent grant with an empty partition is an idempotent no-op.
    let absent = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeOperatorScopes {
            principal,
            revoked: BTreeSet::new(),
            remaining: BTreeSet::new(),
        },
    )
    .expect("absent-grant revoke is a no-op");
    assert!(absent.effects().is_empty());
}

// ==========================================================================
// Subscription third outcome (§6.5)
// ==========================================================================

#[test]
fn subscription_third_outcome_routes_remote_members_and_keeps_peer_only_reject() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    let local_binding = seed_local_member(
        &mut authority,
        "local",
        0,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    seed_remote_member(
        &mut authority,
        "remote",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    // Peer-only member: opened + committed with no bridge session.
    authorize_spawn_profile(&mut authority, "peer-only", 0, None);
    MobMachineMutator::apply(
        &mut authority,
        begin_spawn_local("peer-only", 0, None, SpawnPolicyRuntimeMode::TurnDriven),
    )
    .expect("peer-only BeginSpawnExec accepted");
    MobMachineMutator::apply(
        &mut authority,
        commit_membership_local("peer-only", 0, None, SpawnPolicyRuntimeMode::TurnDriven),
    )
    .expect("peer-only CommitSpawnMembership accepted");

    // Local member: unchanged local authorize outcome.
    let local = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::SubscribeAgentEvents {
            agent_identity: identity("local"),
        },
    )
    .expect("local subscribe accepted");
    assert!(local.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::AuthorizeAgentEventSubscription { session_id, .. }
            if session_id == &local_binding
    )));

    // Remote member: the third outcome, carrying the placement host.
    let remote = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::SubscribeAgentEvents {
            agent_identity: identity("remote"),
        },
    )
    .expect("remote subscribe accepted");
    assert!(remote.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::AuthorizeExternalAgentEventSubscription { agent_identity, host: h }
            if agent_identity == &identity("remote") && h == &host
    )));
    assert!(
        !remote.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::AuthorizeAgentEventSubscription { .. }
        )),
        "a placed member must not receive the local authorize outcome",
    );

    // Peer-only member: unchanged NoSessionBinding reject (the pin).
    let peer_only = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::SubscribeAgentEvents {
            agent_identity: identity("peer-only"),
        },
    )
    .expect("peer-only subscribe classified");
    assert!(peer_only.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::RejectAgentEventSubscription { .. }
    )));

    // SubscribeAll carries the machine-validated external member set.
    let session_bound_runtimes = BTreeSet::from([runtime_id("local", 0), runtime_id("remote", 0)]);
    let external_members = BTreeSet::from([identity("remote")]);
    let all = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::SubscribeAllAgentEvents {
            session_bound_runtimes: session_bound_runtimes.clone(),
            external_members: external_members.clone(),
        },
    )
    .expect("subscribe-all accepted with a consistent external set");
    assert!(all.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::AuthorizeAllAgentEventSubscription { external_members: ext, .. }
            if ext == &external_members
    )));

    // An external set that silently omits a placed member is rejected — the
    // silent cap this seam existed to kill.
    assert!(
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::SubscribeAllAgentEvents {
                session_bound_runtimes,
                external_members: BTreeSet::new(),
            },
        )
        .is_err(),
        "an external-member observation omitting a placed member must be rejected",
    );
}

// ==========================================================================
// Member-operator upcall admission (§15 R6)
// ==========================================================================

#[test]
fn member_operator_admission_covers_accept_and_every_reject_cause() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    seed_remote_member(
        &mut authority,
        "remote",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    seed_local_member(
        &mut authority,
        "local",
        0,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    // Give the local member a registered peer so the sender-key check can
    // pass and the placement check is the discriminating reject.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RegisterMemberPeer {
            agent_identity: identity("local"),
            agent_runtime_id: runtime_id("local", 0),
            generation: Generation(0),
            fence_token: FenceToken(1),
            peer_endpoint: member_endpoint("local", 5),
        },
    )
    .expect("local peer registration accepted");

    let resolve = |authority: &mut MobMachineAuthority,
                   name: &str,
                   generation: u64,
                   fence_token: u64,
                   requester_host: &str,
                   requester_host_binding_generation: u64,
                   requester_session: &str,
                   sender: &str| {
        let transition = MobMachineMutator::apply(
            authority,
            MobMachineInput::ResolveMemberOperatorAdmission {
                agent_identity: identity(name),
                requester_generation: Generation(generation),
                requester_fence_token: FenceToken(fence_token),
                requester_host_id: HostId(requester_host.to_string()),
                requester_host_binding_generation,
                requester_member_session_id: SessionId(requester_session.to_string()),
                sender_peer_id: PeerId(sender.to_string()),
                request_id: "req-1".to_string(),
            },
        )
        .expect("member-operator admission is total");
        transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                MobMachineEffect::MemberOperatorAdmitted { .. } => Some(None),
                MobMachineEffect::MemberOperatorRejected { cause, .. } => Some(Some(*cause)),
                _ => None,
            })
            .expect("admission must emit a verdict")
    };

    assert_eq!(
        resolve(
            &mut authority,
            "remote",
            0,
            1,
            "host-b",
            1,
            "member-host-session-remote",
            "peer-remote",
        ),
        None,
        "a placed member with the matching sender key is admitted",
    );
    assert_eq!(
        resolve(
            &mut authority,
            "ghost",
            1,
            1,
            "host-b",
            1,
            "ghost-session",
            "peer-ghost",
        ),
        Some(MemberOperatorRejectKind::UnknownIdentity),
    );
    assert_eq!(
        resolve(
            &mut authority,
            "remote",
            0,
            1,
            "host-b",
            1,
            "member-host-session-remote",
            "peer-imposter",
        ),
        Some(MemberOperatorRejectKind::SenderKeyMismatch),
    );
    assert_eq!(
        resolve(
            &mut authority,
            "remote",
            1,
            1,
            "host-b",
            1,
            "member-host-session-remote",
            "peer-remote",
        ),
        Some(MemberOperatorRejectKind::StaleGeneration),
    );
    assert_eq!(
        resolve(
            &mut authority,
            "remote",
            0,
            99,
            "host-b",
            1,
            "member-host-session-remote",
            "peer-remote",
        ),
        Some(MemberOperatorRejectKind::StaleFence),
    );
    assert_eq!(
        resolve(
            &mut authority,
            "remote",
            0,
            1,
            "host-b",
            1,
            "stale-member-session",
            "peer-remote",
        ),
        Some(MemberOperatorRejectKind::StaleSession),
    );
    assert_eq!(
        resolve(
            &mut authority,
            "local",
            0,
            1,
            "host-b",
            1,
            "bridge-local-gen0",
            "peer-local",
        ),
        Some(MemberOperatorRejectKind::NoPlacement),
    );
    assert_eq!(
        resolve(
            &mut authority,
            "remote",
            0,
            1,
            "host-other",
            1,
            "member-host-session-remote",
            "peer-remote",
        ),
        Some(MemberOperatorRejectKind::StaleHost),
    );
    assert_eq!(
        resolve(
            &mut authority,
            "remote",
            0,
            1,
            "host-b",
            2,
            "member-host-session-remote",
            "peer-remote",
        ),
        Some(MemberOperatorRejectKind::StaleHostBindingGeneration),
    );
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host.clone(),
            binding_generation: 1,
        },
    )
    .expect("revoke accepted");
    assert_eq!(
        resolve(
            &mut authority,
            "remote",
            0,
            1,
            "host-b",
            1,
            "member-host-session-remote",
            "peer-remote",
        ),
        Some(MemberOperatorRejectKind::HostRevoked),
        "placement survives revoke, so the upcall reject is HostRevoked",
    );
}

// ==========================================================================
// Retirement disposal (§19.L2/L4)
// ==========================================================================

#[test]
fn active_member_archival_without_retire_admission_is_rejected() {
    let mut authority = MobMachineAuthority::new();
    let binding = seed_local_member(
        &mut authority,
        "alpha",
        0,
        SpawnPolicyRuntimeMode::TurnDriven,
    );

    let error = authority
        .apply_signal(MobMachineSignal::ObserveMemberRetirementArchived {
            agent_identity: identity("alpha"),
            agent_runtime_id: runtime_id("alpha", 0),
            fence_token: FenceToken(1),
            generation: Generation(0),
            session_id: Some(binding),
            disposal: MemberSessionDisposal::Archived,
            preserve_machine_topology: false,
        })
        .expect_err("archive observation without Retire admission must fail closed");
    assert!(
        error
            .to_string()
            .contains("guard rejected transition from phase Running"),
        "unexpected rejection: {error}"
    );
    assert!(
        authority
            .state()
            .live_runtime_ids
            .contains(&runtime_id("alpha", 0)),
        "rejected archival must leave the active incarnation untouched"
    );
}

#[test]
fn local_retirement_archival_carries_typed_disposal() {
    let mut authority = MobMachineAuthority::new();
    let binding = seed_local_member(
        &mut authority,
        "alpha",
        0,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    let retirement_started = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::Retire {
            mob_id: MobId("test-mob".to_string()),
            agent_runtime_id: runtime_id("alpha", 0),
            agent_identity: identity("alpha"),
            generation: Generation(0),
            releasing: Some(binding.clone()),
            session_id: Some(binding.clone()),
        },
    )
    .expect("local retire accepted");
    assert!(retirement_started.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::AppendLifecycleJournal {
            kind: MobLifecycleJournalKind::MemberRetirementStartedReleasing,
            agent_identity: Some(agent_identity),
            agent_runtime_id: Some(agent_runtime_id),
            generation: Some(Generation(0)),
            session_id: Some(session_id),
            ..
        } if agent_identity == &identity("alpha")
            && agent_runtime_id == &runtime_id("alpha", 0)
            && session_id == &binding
    )));
    assert!(!retirement_started.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::AppendLifecycleJournal {
            kind: MobLifecycleJournalKind::MemberRetired,
            ..
        }
    )));
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::SessionIngressDetachedForMobDestroy {
            mob_id: MobId("test-mob".to_string()),
            agent_runtime_id: runtime_id("alpha", 0),
        },
    )
    .expect("the exact session-ingress detach proof closes before archival");

    let retired = authority
        .apply_signal(MobMachineSignal::ObserveMemberRetirementArchived {
            agent_identity: identity("alpha"),
            agent_runtime_id: runtime_id("alpha", 0),
            fence_token: FenceToken(1),
            generation: Generation(0),
            session_id: Some(binding.clone()),
            disposal: MemberSessionDisposal::Archived,
            preserve_machine_topology: false,
        })
        .expect("archival observation with a typed disposal accepted");
    assert!(retired.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::AppendLifecycleJournal {
            kind: MobLifecycleJournalKind::MemberRetired,
            agent_identity: Some(agent_identity),
            agent_runtime_id: Some(agent_runtime_id),
            generation: Some(Generation(0)),
            session_id: Some(session_id),
            ..
        } if agent_identity == &identity("alpha")
            && agent_runtime_id == &runtime_id("alpha", 0)
            && session_id == &binding
    )));
    assert!(!retired.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::AppendLifecycleJournal {
            kind: MobLifecycleJournalKind::MemberRetirementStarted,
            ..
        }
    )));
    assert!(
        !authority
            .state()
            .live_runtime_ids
            .contains(&runtime_id("alpha", 0)),
        "archival completes the member teardown",
    );
}

#[test]
fn remote_retire_emits_host_addressed_release_and_destroy_clears_placement() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-b");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    let ack_session = seed_remote_member(
        &mut authority,
        "remote",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );

    // Retire of a placed member goes through the host-addressed release, not
    // the local runtime retire.
    let retire = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::Retire {
            mob_id: MobId("test-mob".to_string()),
            agent_runtime_id: runtime_id("remote", 0),
            agent_identity: identity("remote"),
            generation: Generation(0),
            releasing: Some(ack_session.clone()),
            session_id: Some(ack_session),
        },
    )
    .expect("remote retire accepted");
    let release = retire.effects().iter().find_map(|effect| match effect {
        MobMachineEffect::RequestMemberRelease {
            agent_identity,
            generation,
            host,
            ..
        } => Some((agent_identity.clone(), *generation, host.clone())),
        _ => None,
    });
    assert_eq!(
        release,
        Some((identity("remote"), Generation(0), host.clone())),
        "remote retire must emit the host-addressed release verb",
    );
    assert!(
        !retire.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::RequestRuntimeRetire { .. }
                | MobMachineEffect::RequestSessionIngressDetachForMobDestroy { .. }
        )),
        "a placed member never routes through the local runtime-retire path",
    );
    assert_eq!(
        authority.state().member_placement.get(&identity("remote")),
        Some(&host),
        "non-destroy retirement keeps the placement fact",
    );

    // Destroy-over-remote-members: the destroy archival signal (typed
    // disposal from the ack) clears placement and materialization failures.
    let mut destroy_authority = MobMachineAuthority::new();
    bind_owner_bridge(&mut destroy_authority);
    bind_host(&mut destroy_authority, &host, 1, HostCaps::FULL);
    let ack_session = seed_remote_member(
        &mut destroy_authority,
        "remote",
        0,
        &host,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    begin_lifecycle_quiesce(
        &mut destroy_authority,
        PlacedCompletionLifecycleIntentKind::Destroy,
    );
    destroy_authority
        .apply_signal(MobMachineSignal::AdmitDestroyCleanup)
        .expect("destroy cleanup admitted");
    let destroy_retire = destroy_authority
        .apply_signal(MobMachineSignal::AdmitDestroyMemberRetire {
            mob_id: MobId("test-mob".to_string()),
            agent_identity: identity("remote"),
            agent_runtime_id: runtime_id("remote", 0),
            fence_token: FenceToken(1),
            generation: Generation(0),
            session_id: Some(ack_session.clone()),
        })
        .expect("destroy member retire admitted for the placed member");
    assert!(
        destroy_retire
            .effects()
            .iter()
            .any(|effect| matches!(effect, MobMachineEffect::RequestMemberRelease { .. })),
        "destroy over a placed member releases via the host-addressed verb",
    );
    destroy_authority
        .apply_signal(MobMachineSignal::ObserveDestroyMemberRetirementArchived {
            agent_identity: identity("remote"),
            agent_runtime_id: runtime_id("remote", 0),
            fence_token: FenceToken(1),
            generation: Generation(0),
            session_id: Some(ack_session),
            disposal: MemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions,
        })
        .expect("destroy archival with the ack disposal accepted");
    let state = destroy_authority.state();
    assert!(
        !state.member_placement.contains_key(&identity("remote")),
        "the destroy family clears member_placement",
    );
    assert!(
        !state
            .member_materialization_failures
            .contains_key(&identity("remote"))
    );
    check_multi_host_invariants(state);
}

#[test]
fn destroy_requires_host_authority_to_be_revoked_first() {
    let mut authority = MobMachineAuthority::new();
    let host = host_id("host-destroy-guard");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host, 1, HostCaps::FULL);
    begin_lifecycle_quiesce(&mut authority, PlacedCompletionLifecycleIntentKind::Destroy);
    authority
        .apply_signal(MobMachineSignal::AdmitDestroyCleanup)
        .expect("destroy cleanup admitted");

    MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy)
        .expect_err("destroy must reject while durable host authority remains");

    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host.clone(),
            binding_generation: 1,
        },
    )
    .expect("authenticated shell protocol may commit the host revoke");
    MobMachineMutator::apply(&mut authority, MobMachineInput::Destroy)
        .expect("destroy may commit after every host authority is drained");
    let state = authority.state();
    assert!(state.host_bind_phase.is_empty());
    assert!(state.mob_hosts.is_empty());
    assert!(state.host_authority_epochs.is_empty());
    check_multi_host_invariants(state);
}

// ==========================================================================
// 2-host × 3-member invariant walk (the kernel-test analog of TLC bounds)
// ==========================================================================

#[test]
fn two_hosts_three_members_invariant_walk() {
    let mut authority = MobMachineAuthority::new();
    let host_live = host_id("host-live");
    let host_dark = host_id("host-dark");
    bind_owner_bridge(&mut authority);
    bind_host(&mut authority, &host_live, 1, HostCaps::FULL);
    bind_host(
        &mut authority,
        &host_dark,
        1,
        HostCaps {
            live: false,
            ..HostCaps::FULL
        },
    );
    check_multi_host_invariants(authority.state());
    assert!(
        authority
            .state()
            .host_live_endpoints
            .contains_key(&host_live)
    );
    assert!(
        !authority
            .state()
            .host_live_endpoints
            .contains_key(&host_dark)
    );

    // One local member, two remote members across the two hosts.
    let local_binding = seed_local_member(
        &mut authority,
        "alpha",
        0,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    check_multi_host_invariants(authority.state());
    seed_remote_member(
        &mut authority,
        "beta",
        0,
        &host_live,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    check_multi_host_invariants(authority.state());
    let gamma_session = seed_remote_member(
        &mut authority,
        "gamma",
        0,
        &host_dark,
        SpawnPolicyRuntimeMode::TurnDriven,
    );
    check_multi_host_invariants(authority.state());
    assert_eq!(authority.state().member_placement.len(), 2);
    assert!(
        !authority
            .state()
            .member_placement
            .contains_key(&identity("alpha"))
    );

    // Retire the member on the dark host through the release verb, then
    // archive it with the ack disposal.
    let remote_retirement_started = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::Retire {
            mob_id: MobId("test-mob".to_string()),
            agent_runtime_id: runtime_id("gamma", 0),
            agent_identity: identity("gamma"),
            generation: Generation(0),
            releasing: Some(gamma_session.clone()),
            session_id: Some(gamma_session.clone()),
        },
    )
    .expect("remote retire accepted");
    assert!(
        remote_retirement_started
            .effects()
            .iter()
            .any(|effect| matches!(
                effect,
                MobMachineEffect::AppendLifecycleJournal {
                    kind: MobLifecycleJournalKind::MemberRetirementStartedPreservingBinding,
                    generation: Some(Generation(0)),
                    session_id: Some(session_id),
                    ..
                } if session_id == &gamma_session
            ))
    );
    check_multi_host_invariants(authority.state());
    let remote_retired = authority
        .apply_signal(MobMachineSignal::ObserveMemberRetirementArchived {
            agent_identity: identity("gamma"),
            agent_runtime_id: runtime_id("gamma", 0),
            fence_token: FenceToken(1),
            generation: Generation(0),
            session_id: Some(gamma_session.clone()),
            disposal: MemberSessionDisposal::Archived,
            preserve_machine_topology: false,
        })
        .expect("remote archival accepted with the ack disposal");
    assert!(remote_retired.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::AppendLifecycleJournal {
            kind: MobLifecycleJournalKind::MemberRetired,
            generation: Some(Generation(0)),
            session_id: Some(session_id),
            ..
        } if session_id == &gamma_session
    )));
    check_multi_host_invariants(authority.state());

    // Local member still healthy and locally routed.
    let local = MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::SubscribeAgentEvents {
            agent_identity: identity("alpha"),
        },
    )
    .expect("local subscribe accepted");
    assert!(local.effects().iter().any(|effect| matches!(
        effect,
        MobMachineEffect::AuthorizeAgentEventSubscription { session_id, .. }
            if session_id == &local_binding
    )));

    // Revoking the live host leaves beta's placement (revival ladder), and
    // every invariant still holds.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeHost {
            host_id: host_live,
            binding_generation: 1,
        },
    )
    .expect("revoke accepted");
    check_multi_host_invariants(authority.state());
}

// ==========================================================================
// Phase 5 grant-fact pins (ADJ-P5-2 / ADJ-P5-8)
// ==========================================================================

/// T-MH1 — machine-layer twin of the runtime rotation row: the supervisor
/// rotation input family never touches the grant maps (grants are
/// principal→mob facts, not epoch-scoped — §8:268).
#[test]
fn rotation_transitions_leave_grant_maps_untouched() {
    let mut authority = MobMachineAuthority::new();
    let principal = PrincipalId("console:auditor".to_string());
    let scopes = BTreeSet::from([ControlScope::List, ControlScope::AdminHost]);

    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::GrantOperatorScopes {
            principal: principal.clone(),
            scopes: scopes.clone(),
            expires_at_ms: Some(9_876_543),
        },
    )
    .expect("grant accepted");

    // Seed epoch-0 supervisor authority, then rotate to epoch 1.
    let protocol_version =
        meerkat_mob::machines::mob_machine::SupervisorProtocolVersion("v1".to_string());
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::ProvisionSupervisorAuthority {
            peer_id: PeerId("sup-epoch0".to_string()),
            signing_key: PeerSigningKey::from([1u8; 32]),
            epoch: 0,
            protocol_version: protocol_version.clone(),
        },
    )
    .expect("provision supervisor authority");
    let operation_id = "operator-grants-survive-rotation".to_string();
    let next_peer_id = PeerId("sup-epoch1".to_string());
    let next_signing_key = PeerSigningKey::from([2u8; 32]);
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RecordSupervisorPendingRotation {
            current_peer_id: PeerId("sup-epoch0".to_string()),
            current_epoch: 0,
            current_protocol_version: protocol_version.clone(),
            operation_id: operation_id.clone(),
            pending_peer_id: next_peer_id.clone(),
            pending_signing_key: next_signing_key,
            pending_epoch: 1,
            pending_protocol_version: protocol_version.clone(),
            accepted_peer_ids: BTreeSet::new(),
            active_peer_ids: BTreeSet::new(),
            member_target_names: BTreeMap::new(),
            member_target_addresses: BTreeMap::new(),
        },
    )
    .expect("record pending supervisor rotation");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::CommitSupervisorRotation {
            current_peer_id: PeerId("sup-epoch0".to_string()),
            current_epoch: 0,
            current_protocol_version: protocol_version.clone(),
            operation_id,
            next_peer_id,
            next_signing_key,
            next_epoch: 1,
            next_protocol_version: protocol_version,
        },
    )
    .expect("commit supervisor rotation");

    assert_eq!(
        authority.state().operator_grant_scopes.get(&principal),
        Some(&scopes),
        "rotation must not touch operator_grant_scopes"
    );
    assert_eq!(
        authority.state().operator_grant_expiries.get(&principal),
        Some(&Some(9_876_543)),
        "rotation must not touch operator_grant_expiries"
    );
}

/// T-MH2 — the compensating runtime pin for the missing 1:1 keying DSL
/// invariant (ADJ-P5-8: the invariant itself is ADJ-12-errata territory; the
/// catalog is frozen this phase): across grant / partial-revoke / full-revoke
/// / re-grant sequences, `operator_grant_scopes` and `operator_grant_expiries`
/// hold exactly the same key set at every step.
#[test]
fn grant_inputs_maintain_scope_and_expiry_key_parity_at_every_step() {
    fn assert_key_parity(state: &MobMachineState, step: &str) {
        let scope_keys: BTreeSet<_> = state.operator_grant_scopes.keys().cloned().collect();
        let expiry_keys: BTreeSet<_> = state.operator_grant_expiries.keys().cloned().collect();
        assert_eq!(
            scope_keys, expiry_keys,
            "grant maps must stay key-locked after {step}"
        );
    }

    let mut authority = MobMachineAuthority::new();
    let p1 = PrincipalId("console:p1".to_string());
    let p2 = PrincipalId("console:p2".to_string());
    assert_key_parity(authority.state(), "init");

    // Grant p1 (with expiry) and p2 (without).
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::GrantOperatorScopes {
            principal: p1.clone(),
            scopes: BTreeSet::from([ControlScope::List, ControlScope::Cancel]),
            expires_at_ms: Some(42),
        },
    )
    .expect("grant p1");
    assert_key_parity(authority.state(), "grant p1");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::GrantOperatorScopes {
            principal: p2.clone(),
            scopes: BTreeSet::from([ControlScope::SubscribeEvents]),
            expires_at_ms: None,
        },
    )
    .expect("grant p2");
    assert_key_parity(authority.state(), "grant p2");

    // Full-replace re-grant of p1 changes the value, not the key discipline.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::GrantOperatorScopes {
            principal: p1.clone(),
            scopes: BTreeSet::from([ControlScope::List]),
            expires_at_ms: None,
        },
    )
    .expect("re-grant p1");
    assert_key_parity(authority.state(), "re-grant p1");
    assert_eq!(
        authority.state().operator_grant_expiries.get(&p1),
        Some(&None),
        "full-replace re-grant replaces the expiry row in lockstep"
    );

    // Partial revoke keeps both rows.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::GrantOperatorScopes {
            principal: p1.clone(),
            scopes: BTreeSet::from([ControlScope::List, ControlScope::Cancel]),
            expires_at_ms: Some(77),
        },
    )
    .expect("re-arm p1");
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeOperatorScopes {
            principal: p1.clone(),
            revoked: BTreeSet::from([ControlScope::Cancel]),
            remaining: BTreeSet::from([ControlScope::List]),
        },
    )
    .expect("partial revoke p1");
    assert_key_parity(authority.state(), "partial revoke p1");

    // Full revoke removes BOTH rows.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeOperatorScopes {
            principal: p1.clone(),
            revoked: BTreeSet::from([ControlScope::List]),
            remaining: BTreeSet::new(),
        },
    )
    .expect("full revoke p1");
    assert_key_parity(authority.state(), "full revoke p1");
    assert!(!authority.state().operator_grant_scopes.contains_key(&p1));

    // Absent-grant no-op leaves parity intact.
    MobMachineMutator::apply(
        &mut authority,
        MobMachineInput::RevokeOperatorScopes {
            principal: p1,
            revoked: BTreeSet::new(),
            remaining: BTreeSet::new(),
        },
    )
    .expect("absent revoke no-op");
    assert_key_parity(authority.state(), "absent revoke");
    assert!(authority.state().operator_grant_scopes.contains_key(&p2));
}
