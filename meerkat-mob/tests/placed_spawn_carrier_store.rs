//! Canonical placed-spawn carrier persistence contracts.
//!
//! The store is deliberately mechanical: every write authority in this file
//! comes from a real generated `MobMachine` transition, and both production
//! backends must classify the same exact CAS states.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io;
use std::path::Path;

use meerkat_contracts::wire::{
    PortableDefinitionExtract, PortableMemberSpec, PortableProfile, PortableSpawnOverlay,
    PortableSystemPrompt, PortableToolConfig, WireMobRuntimeMode, WireMobToolAuthorityContext,
    WireSpawnContinuityIntent, portable_member_spec_digest,
};
use meerkat_core::comms::{PeerId as RuntimePeerId, TrustedPeerDescriptor};
use meerkat_core::ops::OperationId;
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationSpec, OperationStatus, OpsLifecycleRegistry,
};
use meerkat_core::{MemberCommsName, Provider, SessionId as RuntimeSessionId};
use meerkat_mob::ids::{MobId, PlacedSpawnId};
use meerkat_mob::machines::mob_machine::{
    AgentIdentity, AgentRuntimeId, FenceToken, Generation, HostId, MemberPeerEndpoint,
    MobMachineAuthority, MobMachineInput, MobMachineMutator, MobMachineSignal, PeerAddress, PeerId,
    PeerName, PeerSigningKey, PlacedCarrierCleanupObligation, PlacedSpawnCarrierExpectedPhase,
    PlacedSpawnId as MachinePlacedSpawnId, SessionId, SpawnPolicyRuntimeMode,
};
use meerkat_mob::store::{
    BeginPlacedSpawnResult, CommitPlacedSpawnResult, DeletePlacedSpawnResult,
    InMemoryMobRuntimeMetadataStore, MobMemberOperatorAuthorityRecord, MobPlacedKickoffIntent,
    MobPlacedSpawnBindingPromotionAuthority, MobPlacedSpawnCarrierRecord,
    MobPlacedSpawnCleanupAuthority, MobPlacedSpawnCommitPersistenceAuthority,
    MobPlacedSpawnPendingPersistenceAuthority, MobRuntimeMetadataStore, PlacedSpawnCarrierPhase,
    PlacedSpawnCommitRecord, PromotePlacedSpawnBindingResult, SqliteMobStores,
};
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
use rusqlite::{Connection, params};
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const MOB_ID: &str = "carrier-contract-mob";
const PROFILE_NAME: &str = "worker";
const PROFILE_MATERIAL_DIGEST: &str = "carrier-profile-material";
const HOST_ENGINE_VERSION: &str = "carrier-test-engine-v1";
const GENERATION: u64 = 0;
const FENCE_TOKEN: u64 = 42;

fn kickoff_intent(prompt: &str) -> MobPlacedKickoffIntent {
    MobPlacedKickoffIntent {
        input_id: uuid::Uuid::new_v4().to_string(),
        objective_id: meerkat_core::interaction::ObjectiveId::new(),
        prompt: meerkat_core::types::ContentInput::Text(prompt.to_string()),
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        injected_context: Vec::new(),
    }
}

struct AttemptWitnesses {
    pending: MobPlacedSpawnCarrierRecord,
    committed: MobPlacedSpawnCarrierRecord,
    pending_write: MobPlacedSpawnPendingPersistenceAuthority,
    commit_write: MobPlacedSpawnCommitPersistenceAuthority,
    pending_cleanup: MobPlacedSpawnCleanupAuthority,
    committed_cleanup: MobPlacedSpawnCleanupAuthority,
}

fn portable_spec(identity: &str) -> PortableMemberSpec {
    PortableMemberSpec {
        mob_id: MOB_ID.to_string(),
        profile_name: PROFILE_NAME.to_string(),
        agent_identity: identity.to_string(),
        profile: PortableProfile {
            model: "claude-opus-4-8".to_string(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: Vec::new(),
            tools: PortableToolConfig {
                comms: true,
                mob: true,
                ..PortableToolConfig::default()
            },
            peer_description: "placed carrier contract worker".to_string(),
            external_addressable: true,
            runtime_mode: WireMobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        },
        definition_extract: PortableDefinitionExtract {
            models: BTreeMap::new(),
            image_generation_provider: None,
            skills: BTreeMap::new(),
            profile_names: vec![PROFILE_NAME.to_string()],
        },
        overlay: PortableSpawnOverlay {
            context: None,
            labels: Some(BTreeMap::from([(
                "contract".to_string(),
                "placed-carrier".to_string(),
            )])),
            additional_instructions: None,
            system_prompt: PortableSystemPrompt::Set {
                text: format!("You are {identity}."),
            },
            tool_access_policy: None,
            mob_tool_authority_context: Some(WireMobToolAuthorityContext {
                principal_token: format!("principal:{identity}"),
                can_create_mobs: true,
                can_mutate_profiles: true,
                can_run_adaptive_packs: false,
                managed_mob_scope: BTreeSet::from([MOB_ID.to_string()]),
                spawn_profile_scope: BTreeMap::from([(
                    MOB_ID.to_string(),
                    BTreeSet::from([PROFILE_NAME.to_string()]),
                )]),
                caller_provenance: None,
                audit_invocation_id: Some(format!("audit:{identity}")),
            }),
            auth_binding: None,
            budget_limits: None,
            runtime_mode: WireMobRuntimeMode::TurnDriven,
            continuity_intent: WireSpawnContinuityIntent::Ephemeral,
        },
        required_env_keys: Vec::new(),
    }
}

fn machine_spawn_id(spawn_id: &PlacedSpawnId) -> MachinePlacedSpawnId {
    MachinePlacedSpawnId(spawn_id.to_string())
}

fn machine_host_id(host_id: RuntimePeerId) -> HostId {
    HostId(host_id.to_string())
}

fn member_endpoint(
    identity: &str,
    key_byte: u8,
    port: u16,
) -> TestResult<(
    MemberPeerEndpoint,
    meerkat_mob::machines::mob_machine::MemberPeerEndpoint,
)> {
    let signing_key = [key_byte; 32];
    let name = MemberCommsName::new(MOB_ID, PROFILE_NAME, identity)?.to_string();
    let peer_id = RuntimePeerId::from_ed25519_pubkey(&signing_key);
    let address = format!("tcp://127.0.0.1:{port}");
    let descriptor = TrustedPeerDescriptor::unsigned_with_pubkey(
        name,
        peer_id.to_string(),
        signing_key,
        address,
    )?;
    let endpoint = MemberPeerEndpoint {
        name: PeerName(descriptor.name.to_string()),
        peer_id: PeerId(descriptor.peer_id.to_string()),
        address: PeerAddress(descriptor.address.to_string()),
        signing_key: PeerSigningKey(descriptor.pubkey),
    };
    Ok((endpoint.clone(), endpoint))
}

fn cleanup_authority(
    record: &MobPlacedSpawnCarrierRecord,
) -> TestResult<MobPlacedSpawnCleanupAuthority> {
    let obligation = PlacedCarrierCleanupObligation {
        agent_identity: AgentIdentity(record.agent_identity.clone()),
        spawn_id: machine_spawn_id(&record.spawn_id),
        generation: Generation(record.generation),
        fence_token: FenceToken(record.fence_token),
        provision_operation_id: record.provision_operation_id.to_string(),
        operation_owner_session_id: SessionId(record.operation_owner_session_id.to_string()),
        expected_phase: match record.phase {
            PlacedSpawnCarrierPhase::Pending => PlacedSpawnCarrierExpectedPhase::Pending,
            PlacedSpawnCarrierPhase::Committed(_) => PlacedSpawnCarrierExpectedPhase::Committed,
        },
    };
    let mut machine = MobMachineAuthority::new();
    machine.apply_signal(MobMachineSignal::RecoverOwnerBridgeSession {
        bridge_session_id: SessionId(record.operation_owner_session_id.to_string()),
        destroy_on_owner_archive: false,
        implicit_delegation_mob: false,
    })?;
    machine.apply_signal(MobMachineSignal::RecoverPlacedCarrierCleanup {
        obligation: obligation.clone(),
    })?;
    let transition = MobMachineMutator::apply(
        &mut machine,
        MobMachineInput::AuthorizePlacedCarrierCleanup { obligation },
    )?;
    Ok(MobPlacedSpawnCleanupAuthority::from_transition(
        record,
        &transition,
    )?)
}

fn binding_promotion_witness(
    expected: &MobPlacedSpawnCarrierRecord,
    host_binding_generation: u64,
) -> TestResult<(
    MobPlacedSpawnCarrierRecord,
    MobPlacedSpawnBindingPromotionAuthority,
)> {
    let PlacedSpawnCarrierPhase::Committed(committed) = &expected.phase else {
        return Err(io::Error::other("promotion fixture requires committed carrier").into());
    };
    let host_id = HostId(expected.host_id.to_string());
    let identity = AgentIdentity(expected.agent_identity.clone());
    let runtime_mode = match expected.spec.profile.runtime_mode {
        WireMobRuntimeMode::AutonomousHost => SpawnPolicyRuntimeMode::AutonomousHost,
        WireMobRuntimeMode::TurnDriven => SpawnPolicyRuntimeMode::TurnDriven,
    };
    let mut machine = MobMachineAuthority::new();
    machine.apply_signal(MobMachineSignal::RecoverOwnerBridgeSession {
        bridge_session_id: SessionId(expected.operation_owner_session_id.to_string()),
        destroy_on_owner_archive: false,
        implicit_delegation_mob: false,
    })?;
    machine.apply_signal(MobMachineSignal::RecoverHostBinding {
        host_id: host_id.clone(),
        pubkey: PeerSigningKey([8; 32]),
        endpoint: PeerAddress("tcp://127.0.0.1:4100".to_string()),
        epoch: host_binding_generation,
        binding_generation: host_binding_generation,
        protocol_min: 4,
        protocol_max: 4,
        engine_version: HOST_ENGINE_VERSION.to_string(),
        durable_sessions: true,
        autonomous_members: true,
        hard_cancel_member: true,
        tracked_input_cancel: true,
        memory_store: true,
        mcp: true,
        resolvable_providers: BTreeSet::from(["anthropic".to_string()]),
        approval_forwarding: false,
        live_endpoint: None,
    })?;
    machine.apply_signal(MobMachineSignal::RecoverCommittedPlacedSpawn {
        spawn_id: machine_spawn_id(&expected.spawn_id),
        agent_identity: identity.clone(),
        agent_runtime_id: AgentRuntimeId(format!(
            "{}:{}",
            expected.agent_identity, expected.generation
        )),
        generation: Generation(expected.generation),
        fence_token: FenceToken(expected.fence_token),
        host_id: host_id.clone(),
        host_binding_generation: expected.host_binding_generation,
        member_session_id: SessionId(committed.member_session_id.to_string()),
        member_peer_endpoint: committed.member_peer_endpoint.clone(),
        profile_name: expected.spec.profile_name.clone(),
        runtime_mode,
        external_addressable: expected.spec.profile.external_addressable,
        provision_operation_id: expected.provision_operation_id.to_string(),
        operation_owner_session_id: SessionId(expected.operation_owner_session_id.to_string()),
    })?;
    let transition = MobMachineMutator::apply(
        &mut machine,
        MobMachineInput::PromoteCommittedPlacedSpawnCarrierBinding {
            spawn_id: machine_spawn_id(&expected.spawn_id),
            agent_identity: identity,
            generation: Generation(expected.generation),
            fence_token: FenceToken(expected.fence_token),
            host_id,
            expected_host_binding_generation: expected.host_binding_generation,
            host_binding_generation,
        },
    )?;
    let mut promoted = expected.clone();
    promoted.host_binding_generation = host_binding_generation;
    let authority =
        MobPlacedSpawnBindingPromotionAuthority::from_transition(expected, &promoted, &transition)?;
    Ok((promoted, authority))
}

#[allow(clippy::too_many_arguments)]
fn attempt_witnesses(
    identity: &str,
    host_id: RuntimePeerId,
    spawn_id: PlacedSpawnId,
    member_session_id: RuntimeSessionId,
    provision_operation_id: OperationId,
    operation_owner_session_id: RuntimeSessionId,
    endpoint_key_byte: u8,
    endpoint_port: u16,
    effective_profile_override_present: bool,
    effective_model_override_present: bool,
    kickoff_intent: Option<MobPlacedKickoffIntent>,
) -> TestResult<AttemptWitnesses> {
    let mut spec = portable_spec(identity);
    let runtime_mode = if kickoff_intent.is_some() {
        spec.profile.runtime_mode = WireMobRuntimeMode::AutonomousHost;
        spec.overlay.runtime_mode = WireMobRuntimeMode::AutonomousHost;
        SpawnPolicyRuntimeMode::AutonomousHost
    } else {
        SpawnPolicyRuntimeMode::TurnDriven
    };
    let spec_digest = portable_member_spec_digest(&spec)?;
    let pending = MobPlacedSpawnCarrierRecord::pending(
        spawn_id.clone(),
        identity.to_string(),
        GENERATION,
        FENCE_TOKEN,
        provision_operation_id.clone(),
        operation_owner_session_id.clone(),
        host_id,
        1,
        spec_digest.clone(),
        spec,
        kickoff_intent,
        effective_profile_override_present,
        effective_model_override_present,
    );
    pending.validate_for_mob(&MobId::from(MOB_ID))?;

    let machine_identity = AgentIdentity(identity.to_string());
    let machine_runtime_id = AgentRuntimeId(format!("{identity}:{GENERATION}"));
    let machine_host = machine_host_id(host_id);
    let mut machine = MobMachineAuthority::new();
    machine.apply_signal(MobMachineSignal::RecoverHostBinding {
        host_id: machine_host.clone(),
        pubkey: PeerSigningKey([8; 32]),
        endpoint: PeerAddress("tcp://127.0.0.1:4100".to_string()),
        epoch: 1,
        binding_generation: 1,
        protocol_min: 4,
        protocol_max: 4,
        engine_version: HOST_ENGINE_VERSION.to_string(),
        durable_sessions: true,
        autonomous_members: true,
        hard_cancel_member: true,
        tracked_input_cancel: true,
        memory_store: true,
        mcp: true,
        resolvable_providers: BTreeSet::from(["anthropic".to_string()]),
        approval_forwarding: false,
        live_endpoint: None,
    })?;
    machine.apply_signal(MobMachineSignal::RecoverOwnerBridgeSession {
        bridge_session_id: SessionId(operation_owner_session_id.to_string()),
        destroy_on_owner_archive: false,
        implicit_delegation_mob: false,
    })?;
    MobMachineMutator::apply(
        &mut machine,
        MobMachineInput::AuthorizeSpawnProfile {
            agent_identity: machine_identity.clone(),
            profile_name: PROFILE_NAME.to_string(),
            model: pending.spec.profile.model.clone(),
            profile_material_digest: PROFILE_MATERIAL_DIGEST.to_string(),
            tool_config_digest: "carrier-tool-digest".to_string(),
            skills_digest: "carrier-skills-digest".to_string(),
            provider_params_digest: None,
            output_schema_digest: None,
            external_addressable: true,
            resolved_spec_digest: Some(spec_digest.clone()),
        },
    )?;
    let begin_transition = MobMachineMutator::apply(
        &mut machine,
        MobMachineInput::BeginSpawnExec {
            agent_identity: machine_identity.clone(),
            agent_runtime_id: machine_runtime_id.clone(),
            fence_token: FenceToken(FENCE_TOKEN),
            generation: Generation(GENERATION),
            profile_material_digest: PROFILE_MATERIAL_DIGEST.to_string(),
            external_addressable: true,
            runtime_mode,
            bridge_session_id: None,
            replacing: None,
            placement: Some(machine_host),
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
            placed_spawn_id: Some(machine_spawn_id(&spawn_id)),
            placed_provision_operation_id: Some(provision_operation_id.to_string()),
            placed_operation_owner_session_id: Some(SessionId(
                operation_owner_session_id.to_string(),
            )),
            effective_profile_override_present,
            effective_model_override_present,
        },
    )?;
    let pending_write =
        MobPlacedSpawnPendingPersistenceAuthority::from_transition(&pending, &begin_transition)?;

    let (member_peer_endpoint, machine_member_peer_endpoint) =
        member_endpoint(identity, endpoint_key_byte, endpoint_port)?;
    let committed = MobPlacedSpawnCarrierRecord {
        phase: PlacedSpawnCarrierPhase::Committed(PlacedSpawnCommitRecord {
            member_session_id: member_session_id.clone(),
            member_peer_endpoint,
            ack_engine_version: HOST_ENGINE_VERSION.to_string(),
        }),
        ..pending.clone()
    };
    committed.validate_for_mob(&MobId::from(MOB_ID))?;
    let commit_transition = MobMachineMutator::apply(
        &mut machine,
        MobMachineInput::CommitSpawnMembership {
            agent_identity: machine_identity,
            agent_runtime_id: machine_runtime_id,
            fence_token: FenceToken(FENCE_TOKEN),
            generation: Generation(GENERATION),
            profile_material_digest: PROFILE_MATERIAL_DIGEST.to_string(),
            external_addressable: true,
            runtime_mode,
            bridge_session_id: Some(SessionId(member_session_id.to_string())),
            replacing: None,
            member_peer_endpoint: Some(machine_member_peer_endpoint),
            spec_digest_echo: Some(spec_digest),
            ack_engine_version: Some(HOST_ENGINE_VERSION.to_string()),
            placed_spawn_id: Some(machine_spawn_id(&spawn_id)),
            provision_operation_id: Some(provision_operation_id.to_string()),
        },
    )?;
    let commit_write =
        MobPlacedSpawnCommitPersistenceAuthority::from_transition(&committed, &commit_transition)?;

    Ok(AttemptWitnesses {
        pending_cleanup: cleanup_authority(&pending)?,
        committed_cleanup: cleanup_authority(&committed)?,
        pending,
        committed,
        pending_write,
        commit_write,
    })
}

fn expected_operator_authority(identity: &str) -> MobMemberOperatorAuthorityRecord {
    MobMemberOperatorAuthorityRecord {
        agent_identity: identity.to_string(),
        generation: GENERATION,
        can_create_mobs: true,
        can_mutate_profiles: true,
        can_run_adaptive_packs: false,
        managed_mob_scope: BTreeSet::from([MOB_ID.to_string()]),
        spawn_profile_scope: BTreeMap::from([(
            MOB_ID.to_string(),
            BTreeSet::from([PROFILE_NAME.to_string()]),
        )]),
    }
}

async fn exercise_canonical_carrier_store(store: &dyn MobRuntimeMetadataStore) -> TestResult {
    let mob_id = MobId::from(MOB_ID);
    let host_id = RuntimePeerId::new();
    let primary_spawn_id = PlacedSpawnId::new();
    let primary_kickoff = kickoff_intent("durable placed kickoff");
    let primary = attempt_witnesses(
        "worker-z",
        host_id,
        primary_spawn_id.clone(),
        RuntimeSessionId::new(),
        OperationId::new(),
        RuntimeSessionId::new(),
        17,
        4201,
        true,
        true,
        Some(primary_kickoff.clone()),
    )?;

    let mut missing_kickoff = primary.pending.clone();
    missing_kickoff.kickoff_intent = None;
    assert!(
        missing_kickoff.validate_for_mob(&mob_id).is_err(),
        "an autonomous carrier must retain its private kickoff intent"
    );

    let mut noncanonical_kickoff = primary.pending.clone();
    noncanonical_kickoff
        .kickoff_intent
        .as_mut()
        .ok_or_else(|| io::Error::other("autonomous carrier lost kickoff intent"))?
        .input_id = "550E8400-E29B-41D4-A716-446655440000".to_string();
    assert!(
        noncanonical_kickoff.validate_for_mob(&mob_id).is_err(),
        "kickoff replay identity must use canonical UUID text"
    );

    let mut steer_kickoff = primary.pending.clone();
    steer_kickoff
        .kickoff_intent
        .as_mut()
        .ok_or_else(|| io::Error::other("autonomous carrier lost kickoff intent"))?
        .handling_mode = meerkat_core::types::HandlingMode::Steer;
    assert!(
        steer_kickoff.validate_for_mob(&mob_id).is_err(),
        "placed kickoff replay is always queue-admitted"
    );

    // The generated persistence witness binds the presence bit. Its exact
    // opposite is otherwise a valid carrier, so rejection proves this is not
    // accidental validation fallout.
    let mut opposite_presence = primary.pending.clone();
    opposite_presence.effective_profile_override_present = false;
    opposite_presence.validate_for_mob(&mob_id)?;
    assert!(
        store
            .begin_placed_spawn_if_absent(&mob_id, &opposite_presence, &primary.pending_write,)
            .await
            .is_err(),
        "a generated true-presence witness must reject the false-presence carrier"
    );
    assert_eq!(
        store.load_placed_spawn(&mob_id, "worker-z").await?,
        None,
        "a rejected witness must not write"
    );

    let mut opposite_model_presence = primary.pending.clone();
    opposite_model_presence.effective_model_override_present = false;
    opposite_model_presence.validate_for_mob(&mob_id)?;
    assert!(
        store
            .begin_placed_spawn_if_absent(
                &mob_id,
                &opposite_model_presence,
                &primary.pending_write,
            )
            .await
            .is_err(),
        "a generated model-override-presence witness must reject the opposite carrier"
    );
    assert_eq!(
        store.load_placed_spawn(&mob_id, "worker-z").await?,
        None,
        "a rejected model-presence witness must not write"
    );

    assert_eq!(
        store
            .begin_placed_spawn_if_absent(&mob_id, &primary.pending, &primary.pending_write)
            .await?,
        BeginPlacedSpawnResult::Inserted
    );
    assert_eq!(
        store
            .begin_placed_spawn_if_absent(&mob_id, &primary.pending, &primary.pending_write)
            .await?,
        BeginPlacedSpawnResult::ExistingExactPending
    );
    let mut changed_kickoff = primary.pending.clone();
    let changed_intent = changed_kickoff
        .kickoff_intent
        .as_mut()
        .ok_or_else(|| io::Error::other("autonomous carrier lost kickoff intent"))?;
    changed_intent.prompt = meerkat_core::types::ContentInput::Text(
        "same attempt must not rename the kickoff".to_string(),
    );
    changed_kickoff.validate_for_mob(&mob_id)?;
    assert_eq!(
        store
            .begin_placed_spawn_if_absent(&mob_id, &changed_kickoff, &primary.pending_write)
            .await?,
        BeginPlacedSpawnResult::Conflict,
        "one placed attempt may not replay with different model-visible kickoff content"
    );
    assert_eq!(
        store
            .load_committed_placed_member_operator_authority(&mob_id, "worker-z", GENERATION,)
            .await?,
        None,
        "Pending carriers never grant member-operator authority"
    );

    let conflicting_attempt = attempt_witnesses(
        "worker-z",
        host_id,
        PlacedSpawnId::new(),
        RuntimeSessionId::new(),
        OperationId::new(),
        RuntimeSessionId::new(),
        18,
        4202,
        true,
        true,
        None,
    )?;
    assert_eq!(
        store
            .begin_placed_spawn_if_absent(
                &mob_id,
                &conflicting_attempt.pending,
                &conflicting_attempt.pending_write,
            )
            .await?,
        BeginPlacedSpawnResult::Conflict,
        "an identity row may not be overwritten by another spawn attempt"
    );

    assert_eq!(
        store
            .compare_and_commit_placed_spawn(
                &mob_id,
                &primary.pending,
                &primary.committed,
                &primary.commit_write,
            )
            .await?,
        CommitPlacedSpawnResult::Committed
    );
    assert_eq!(
        store
            .load_placed_spawn(&mob_id, "worker-z")
            .await?
            .and_then(|record| record.kickoff_intent),
        Some(primary_kickoff.clone()),
        "pending-to-committed CAS must preserve exact kickoff replay material"
    );
    assert_eq!(
        store
            .compare_and_commit_placed_spawn(
                &mob_id,
                &primary.pending,
                &primary.committed,
                &primary.commit_write,
            )
            .await?,
        CommitPlacedSpawnResult::AlreadyCommittedExact,
        "lost commit ACK replay must classify the exact committed row"
    );
    assert_eq!(
        store
            .begin_placed_spawn_if_absent(&mob_id, &primary.pending, &primary.pending_write)
            .await?,
        BeginPlacedSpawnResult::ExistingExactCommitted
    );
    assert_eq!(
        store
            .load_committed_placed_member_operator_authority(&mob_id, "worker-z", GENERATION,)
            .await?,
        Some(expected_operator_authority("worker-z")),
        "only the committed carrier projects member-operator authority"
    );
    assert_eq!(
        store
            .load_committed_placed_member_operator_authority(&mob_id, "worker-z", GENERATION + 1,)
            .await?,
        None,
        "operator authority is generation-exact"
    );

    // Same attempt tuple, different authenticated ACK facts. The alternate
    // write permit is also generated, so the backend—not fixture authority
    // validation—must classify this as a stale/conflicting commit.
    let stale_ack = attempt_witnesses(
        "worker-z",
        host_id,
        primary_spawn_id,
        RuntimeSessionId::new(),
        primary.pending.provision_operation_id.clone(),
        primary.pending.operation_owner_session_id.clone(),
        19,
        4203,
        true,
        true,
        Some(primary_kickoff.clone()),
    )?;
    assert_eq!(stale_ack.pending, primary.pending);
    assert_ne!(stale_ack.committed, primary.committed);
    assert_eq!(
        store
            .compare_and_commit_placed_spawn(
                &mob_id,
                &stale_ack.pending,
                &stale_ack.committed,
                &stale_ack.commit_write,
            )
            .await?,
        CommitPlacedSpawnResult::Conflict
    );

    let (primary_promoted, promote_write) = binding_promotion_witness(&primary.committed, 2)?;
    assert_eq!(
        store
            .compare_and_promote_placed_spawn_binding(
                &mob_id,
                &primary.committed,
                &primary_promoted,
                &promote_write,
            )
            .await?,
        PromotePlacedSpawnBindingResult::Promoted
    );
    assert_eq!(
        store
            .compare_and_promote_placed_spawn_binding(
                &mob_id,
                &primary.committed,
                &primary_promoted,
                &promote_write,
            )
            .await?,
        PromotePlacedSpawnBindingResult::AlreadyPromotedExact,
        "crash after carrier CAS but before machine commit must replay idempotently"
    );
    let (skipped_generation, skipped_generation_write) =
        binding_promotion_witness(&primary.committed, 3)?;
    assert_eq!(
        store
            .compare_and_promote_placed_spawn_binding(
                &mob_id,
                &primary.committed,
                &skipped_generation,
                &skipped_generation_write,
            )
            .await?,
        PromotePlacedSpawnBindingResult::Conflict,
        "a stale G1 CAS cannot overwrite the already-promoted G2 row"
    );
    let (exact_generation_three, generation_three_write) =
        binding_promotion_witness(&primary_promoted, 3)?;
    let mut ack_drift = exact_generation_three.clone();
    let PlacedSpawnCarrierPhase::Committed(commit) = &mut ack_drift.phase else {
        unreachable!("promotion fixture remains committed")
    };
    commit.ack_engine_version.push_str("-drift");
    assert!(
        store
            .compare_and_promote_placed_spawn_binding(
                &mob_id,
                &primary_promoted,
                &ack_drift,
                &generation_three_write,
            )
            .await
            .is_err(),
        "promotion authority must reject any ACK/logical carrier mutation"
    );
    assert_eq!(
        store.load_placed_spawn(&mob_id, "worker-z").await?,
        Some(primary_promoted.clone()),
        "rejected promotion must roll back without changing the durable row"
    );
    assert_eq!(
        primary_promoted.kickoff_intent,
        Some(primary_kickoff),
        "host-binding promotion must not rewrite kickoff replay material"
    );

    let alphabetically_first = attempt_witnesses(
        "worker-a",
        host_id,
        PlacedSpawnId::new(),
        RuntimeSessionId::new(),
        OperationId::new(),
        RuntimeSessionId::new(),
        20,
        4204,
        false,
        false,
        None,
    )?;
    assert_eq!(
        store
            .begin_placed_spawn_if_absent(
                &mob_id,
                &alphabetically_first.pending,
                &alphabetically_first.pending_write,
            )
            .await?,
        BeginPlacedSpawnResult::Inserted
    );
    let listed = store.list_placed_spawns(&mob_id).await?;
    assert_eq!(
        listed
            .iter()
            .map(|record| record.agent_identity.as_str())
            .collect::<Vec<_>>(),
        vec!["worker-a", "worker-z"],
        "carrier recovery enumeration is deterministic by identity"
    );

    assert_eq!(
        store
            .compare_and_delete_placed_spawn(&mob_id, &primary.pending, &primary.pending_cleanup,)
            .await?,
        DeletePlacedSpawnResult::Conflict,
        "a Pending cleanup obligation cannot delete the committed phase"
    );
    assert_eq!(
        store
            .compare_and_delete_placed_spawn(
                &mob_id,
                &primary_promoted,
                &cleanup_authority(&primary_promoted)?,
            )
            .await?,
        DeletePlacedSpawnResult::Deleted
    );
    assert_eq!(
        store
            .compare_and_delete_placed_spawn(
                &mob_id,
                &primary_promoted,
                &cleanup_authority(&primary_promoted)?,
            )
            .await?,
        DeletePlacedSpawnResult::AlreadyAbsent,
        "lost delete ACK replay is exact and idempotent"
    );
    assert_eq!(
        store
            .compare_and_delete_placed_spawn(
                &mob_id,
                &alphabetically_first.pending,
                &alphabetically_first.pending_cleanup,
            )
            .await?,
        DeletePlacedSpawnResult::Deleted
    );
    assert!(store.list_placed_spawns(&mob_id).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn in_memory_store_obeys_canonical_placed_spawn_cas_contract() -> TestResult {
    exercise_canonical_carrier_store(&InMemoryMobRuntimeMetadataStore::new()).await
}

#[tokio::test]
async fn sqlite_store_obeys_canonical_placed_spawn_cas_contract() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("carrier-contract.sqlite");
    let store = SqliteMobStores::open(&path)?.runtime_metadata_store();
    exercise_canonical_carrier_store(&store).await
}

fn raw_insert_carrier(path: &Path, value: &Value) -> TestResult {
    let agent_identity = value
        .get("agent_identity")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("carrier fixture lacks agent_identity"))?;
    let connection = Connection::open(path)?;
    connection.execute(
        "INSERT INTO mob_runtime_placed_spawns (mob_id, agent_identity, record_json) \
         VALUES (?1, ?2, ?3)",
        params![MOB_ID, agent_identity, serde_json::to_vec(value)?,],
    )?;
    Ok(())
}

#[test]
fn stale_cleanup_authority_is_rejected_before_successor_operation_mutation() -> TestResult {
    let host = RuntimePeerId::new();
    let stale = attempt_witnesses(
        "worker-order",
        host,
        PlacedSpawnId::new(),
        RuntimeSessionId::new(),
        OperationId::new(),
        RuntimeSessionId::new(),
        31,
        4301,
        false,
        false,
        None,
    )?;
    let successor = attempt_witnesses(
        "worker-order",
        host,
        PlacedSpawnId::new(),
        RuntimeSessionId::new(),
        OperationId::new(),
        RuntimeSessionId::new(),
        32,
        4302,
        false,
        false,
        None,
    )?;
    let registry = RuntimeOpsLifecycleRegistry::new();
    let display_name = MemberCommsName::new(MOB_ID, PROFILE_NAME, "worker-order")?.to_string();
    registry.register_operation(OperationSpec {
        id: successor.committed.provision_operation_id.clone(),
        kind: OperationKind::MobMemberChild,
        owner_session_id: successor.committed.operation_owner_session_id.clone(),
        display_name,
        source_label: "mob_member".to_string(),
        operation_source: None,
        child_session_id: None,
        expect_peer_channel: true,
    })?;
    registry.provisioning_succeeded(&successor.committed.provision_operation_id)?;

    assert!(
        stale
            .committed_cleanup
            .verify_record(&successor.committed)
            .is_err(),
        "a stale obligation must fail full carrier authorization"
    );
    let snapshot = registry
        .snapshot(&successor.committed.provision_operation_id)?
        .ok_or_else(|| io::Error::other("successor operation disappeared"))?;
    assert_eq!(snapshot.status, OperationStatus::Running);
    assert!(
        !snapshot.terminal,
        "authorization failure mutated successor op"
    );
    Ok(())
}

async fn assert_sqlite_backend_rejects(value: Value, expected_error_fragment: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("strict-carrier.sqlite");
    let store = SqliteMobStores::open(&path)?.runtime_metadata_store();
    raw_insert_carrier(&path, &value)?;
    let error = match store
        .load_placed_spawn(&MobId::from(MOB_ID), "worker-z")
        .await
    {
        Ok(record) => {
            return Err(io::Error::other(format!(
                "malformed durable carrier was accepted: {record:?}"
            ))
            .into());
        }
        Err(error) => error,
    };
    assert!(
        error.to_string().contains(expected_error_fragment),
        "unexpected backend decode error: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn sqlite_v4_carrier_defaults_absent_model_override_presence_to_false() -> TestResult {
    let attempt = attempt_witnesses(
        "worker-legacy-model",
        RuntimePeerId::new(),
        PlacedSpawnId::new(),
        RuntimeSessionId::new(),
        OperationId::new(),
        RuntimeSessionId::new(),
        22,
        4206,
        false,
        false,
        None,
    )?;
    let mut current_v4 = serde_json::to_value(&attempt.pending)?;
    assert_eq!(
        current_v4["schema_version"],
        serde_json::json!(4),
        "kickoff replay material advances the canonical carrier schema"
    );
    let removed = current_v4
        .as_object_mut()
        .ok_or_else(|| io::Error::other("pending carrier did not serialize as an object"))?
        .remove("effective_model_override_present");
    assert_eq!(removed, Some(serde_json::json!(false)));

    let decoded: MobPlacedSpawnCarrierRecord = serde_json::from_value(current_v4.clone())?;
    assert!(!decoded.effective_model_override_present);
    assert_eq!(decoded.rehydrated_effective_model_override(), None);
    decoded.validate_for_mob(&MobId::from(MOB_ID))?;

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("v4-model-override.sqlite");
    let store = SqliteMobStores::open(&path)?.runtime_metadata_store();
    raw_insert_carrier(&path, &current_v4)?;
    let loaded = store
        .load_placed_spawn(&MobId::from(MOB_ID), "worker-legacy-model")
        .await?
        .ok_or_else(|| io::Error::other("v4 carrier was not loaded"))?;
    assert!(!loaded.effective_model_override_present);
    assert_eq!(loaded.rehydrated_effective_model_override(), None);
    Ok(())
}

#[tokio::test]
async fn sqlite_carrier_decode_rejects_schema_and_serde_drift() -> TestResult {
    let attempt = attempt_witnesses(
        "worker-z",
        RuntimePeerId::new(),
        PlacedSpawnId::new(),
        RuntimeSessionId::new(),
        OperationId::new(),
        RuntimeSessionId::new(),
        21,
        4205,
        true,
        true,
        None,
    )?;

    let mut unknown_schema = serde_json::to_value(&attempt.pending)?;
    unknown_schema["schema_version"] = serde_json::json!(999);
    let decoded_unknown_schema: MobPlacedSpawnCarrierRecord =
        serde_json::from_value(unknown_schema.clone())?;
    assert!(decoded_unknown_schema.validate().is_err());
    assert_sqlite_backend_rejects(
        unknown_schema,
        "unsupported placed-spawn carrier schema_version",
    )
    .await?;

    let mut missing_presence = serde_json::to_value(&attempt.pending)?;
    missing_presence
        .as_object_mut()
        .ok_or_else(|| io::Error::other("pending carrier did not serialize as an object"))?
        .remove("effective_profile_override_present");
    assert!(
        serde_json::from_value::<MobPlacedSpawnCarrierRecord>(missing_presence.clone()).is_err(),
        "the presence bit is required, not serde-defaulted"
    );
    assert_sqlite_backend_rejects(missing_presence, "effective_profile_override_present").await?;

    let mut missing_binding_generation = serde_json::to_value(&attempt.pending)?;
    missing_binding_generation
        .as_object_mut()
        .ok_or_else(|| io::Error::other("pending carrier did not serialize as an object"))?
        .remove("host_binding_generation");
    assert!(
        serde_json::from_value::<MobPlacedSpawnCarrierRecord>(missing_binding_generation.clone())
            .is_err(),
        "v4 host_binding_generation is required, not serde-defaulted"
    );
    assert_sqlite_backend_rejects(missing_binding_generation, "host_binding_generation").await?;

    let mut zero_binding_generation = attempt.pending.clone();
    zero_binding_generation.host_binding_generation = 0;
    assert!(
        zero_binding_generation.validate().is_err(),
        "v4 carrier binding generation must be nonzero"
    );

    let mut unknown_outer = serde_json::to_value(&attempt.pending)?;
    unknown_outer
        .as_object_mut()
        .ok_or_else(|| io::Error::other("pending carrier did not serialize as an object"))?
        .insert("future_outer_fact".to_string(), serde_json::json!(true));
    assert!(
        serde_json::from_value::<MobPlacedSpawnCarrierRecord>(unknown_outer.clone()).is_err(),
        "unknown outer facts must fail serde"
    );
    assert_sqlite_backend_rejects(unknown_outer, "unknown field").await?;

    let mut unknown_nested_commit = serde_json::to_value(&attempt.committed)?;
    unknown_nested_commit
        .pointer_mut("/phase/commit")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| io::Error::other("committed carrier lacks nested commit object"))?
        .insert("future_ack_fact".to_string(), serde_json::json!(true));
    assert!(
        serde_json::from_value::<MobPlacedSpawnCarrierRecord>(unknown_nested_commit.clone())
            .is_err(),
        "unknown authenticated ACK facts must fail serde"
    );
    assert_sqlite_backend_rejects(unknown_nested_commit, "unknown field").await?;
    Ok(())
}
