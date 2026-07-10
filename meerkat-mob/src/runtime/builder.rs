use super::*;
use crate::store::authority_validating_mob_run_store;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::comms::{
    CommsTrustMutation, CommsTrustMutationAuthority, CommsTrustMutationResult, PeerId, SendError,
    TrustedPeerDescriptor,
};
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::collections::HashSet;

const MOB_COMMAND_CHANNEL_CAPACITY: usize = 4096;

struct ResumeDesiredTrust {
    spec: TrustedPeerDescriptor,
    source: ResumeTrustSource,
}

struct ResumeTrustMutation {
    comms: Arc<dyn CoreCommsRuntime>,
    operation: ResumeTrustMutationOperation,
    authority: CommsTrustMutationAuthority,
}

enum ResumeTrustMutationOperation {
    Add(TrustedPeerDescriptor),
    Remove(String),
}

enum ResumeTrustSource {
    Member(crate::machines::mob_machine::WiringEdge),
    External {
        key: crate::machines::mob_machine::ExternalPeerKey,
        edge: crate::machines::mob_machine::ExternalPeerEdge,
    },
}

fn recovered_endpoint_runtime_is_retiring(
    state: &crate::machines::mob_machine::MobMachineState,
    identity: &crate::machines::mob_machine::AgentIdentity,
) -> bool {
    let Some(runtime_id) = state.identity_to_runtime.get(identity) else {
        return false;
    };
    state.member_state_markers.get(runtime_id)
        == Some(&crate::machines::mob_machine::MobMemberState::Retiring)
}

fn recovered_member_edge_allows_trust_repair(
    state: &crate::machines::mob_machine::MobMachineState,
    edge: &crate::machines::mob_machine::WiringEdge,
) -> bool {
    !recovered_endpoint_runtime_is_retiring(state, &edge.a)
        && !recovered_endpoint_runtime_is_retiring(state, &edge.b)
}

fn recovered_peer_only_overlay_allows_trust_reconcile(
    state: &crate::machines::mob_machine::MobMachineState,
    member_edges: &std::collections::BTreeSet<crate::machines::mob_machine::WiringEdge>,
    identity: &crate::machines::mob_machine::AgentIdentity,
) -> bool {
    !recovered_endpoint_runtime_is_retiring(state, identity)
        && member_edges.iter().all(|edge| {
            (edge.a != *identity && edge.b != *identity)
                || recovered_member_edge_allows_trust_repair(state, edge)
        })
}

// ---------------------------------------------------------------------------
// MobBuilder
// ---------------------------------------------------------------------------

/// Builder for creating or resuming a mob.
pub struct MobBuilder {
    mode: BuilderMode,
    storage: MobStorage,
    session_service: Option<Arc<dyn MobSessionService>>,
    #[cfg(feature = "runtime-adapter")]
    runtime_adapter: RuntimeAdapterOption,
    workgraph_service: Option<meerkat::WorkGraphService>,
    allow_ephemeral_sessions: bool,
    notify_orchestrator_on_resume: bool,
    tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    default_external_tools_provider: Option<crate::ExternalToolsProvider>,
    spawn_member_customizer: Option<Arc<dyn super::SpawnMemberCustomizer>>,
    owner_bridge_session_create_authority: Option<OwnerBridgeSessionCreateAuthority>,
    /// Optional realtime session factory injected for mob-provisioned
    /// members that open a realtime channel (W2-E / issue #264).
    ///
    /// Today this field is a carrier: tests can set it via
    /// [`MobBuilder::with_realtime_session_factory`] and retrieve it from
    /// [`MobHandle::realtime_session_factory`] to wire into a
    /// `RealtimeWsHost` bound to the same runtime. Follow-up work lands a
    /// direct mob→host plumbing path so `RealtimeAttached` cells in the
    /// W1-C coverage matrix can be exercised end-to-end without per-test
    /// TCP orchestration.
    realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
}

enum BuilderMode {
    Create(Arc<MobDefinition>),
    Resume,
}

#[derive(Clone)]
struct OwnerBridgeSessionCreateAuthority {
    bridge_session_id: SessionId,
    destroy_on_owner_archive: bool,
    implicit_delegation_mob: bool,
}

fn seed_mob_authority() -> crate::machines::mob_machine::MobMachineAuthority {
    crate::machines::mob_machine::MobMachineAuthority::new()
}

fn apply_seeded_mob_input_transition(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    input: crate::machines::mob_machine::MobMachineInput,
    context: &'static str,
) -> Result<crate::machines::mob_machine::MobMachineTransition, MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let input_debug = format!("{input:?}");
    let transition = mob_dsl::MobMachineMutator::apply(authority, input).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine seeded authority ({context}) rejected {input_debug}: {error}"
        ))
    })?;
    Ok(transition)
}

fn apply_seeded_mob_input(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    input: crate::machines::mob_machine::MobMachineInput,
    context: &'static str,
) -> Result<(), MobError> {
    let _ = apply_seeded_mob_input_transition(authority, input, context)?;
    Ok(())
}

fn bind_owner_bridge_session_authority_for_create(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    binding: &OwnerBridgeSessionCreateAuthority,
    context: &'static str,
) -> Result<Option<MobEventKind>, MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let transition = apply_seeded_mob_input_transition(
        authority,
        mob_dsl::MobMachineInput::BindOwnerBridgeSession {
            bridge_session_id: mob_dsl::SessionId::from_domain(&binding.bridge_session_id),
            destroy_on_owner_archive: binding.destroy_on_owner_archive,
            implicit_delegation_mob: binding.implicit_delegation_mob,
        },
        context,
    )?;
    let mut generated = None;
    for effect in transition.effects() {
        if let mob_dsl::MobMachineEffect::OwnerBridgeSessionBound {
            bridge_session_id,
            destroy_on_owner_archive,
            implicit_delegation_mob,
        } = effect
        {
            if generated.is_some() {
                return Err(MobError::Internal(format!(
                    "MobMachine seeded authority ({context}) emitted multiple owner bridge-session bindings"
                )));
            }
            let bridge_session_id =
                SessionId::parse(&bridge_session_id.0).map_err(|error| {
                    MobError::Internal(format!(
                        "MobMachine seeded authority ({context}) emitted invalid owner bridge session id: {error}"
                    ))
                })?;
            generated = Some(MobEventKind::MobOwnerBridgeSessionBound {
                bridge_session_id,
                destroy_on_owner_archive: *destroy_on_owner_archive,
                implicit_delegation_mob: *implicit_delegation_mob,
            });
        }
    }
    if generated.is_none() {
        return Err(MobError::Internal(format!(
            "MobMachine seeded authority ({context}) emitted no owner bridge-session binding"
        )));
    }
    Ok(generated)
}

fn register_seeded_member_peer(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    agent_identity: &crate::ids::AgentIdentity,
    peer_descriptor: &meerkat_core::comms::TrustedPeerDescriptor,
    context: &'static str,
) -> Result<(), MobError> {
    apply_seeded_mob_input(
        authority,
        crate::machines::mob_machine::MobMachineInput::RegisterMemberPeer {
            agent_identity: crate::machines::mob_machine::AgentIdentity::from_domain(
                agent_identity,
            ),
            peer_endpoint: crate::machines::mob_machine::MemberPeerEndpoint::from(peer_descriptor),
        },
        context,
    )
}

fn member_peer_rebind_endpoint_from_transition(
    transition: &crate::machines::mob_machine::MobMachineTransition,
    agent_identity: &crate::machines::mob_machine::AgentIdentity,
    context: &'static str,
) -> Result<crate::machines::mob_machine::MemberPeerEndpoint, MobError> {
    let mut authorized = None;
    for effect in transition.effects() {
        if let crate::machines::mob_machine::MobMachineEffect::MemberPeerRebindAuthorized {
            agent_identity: effect_identity,
            peer_endpoint,
            ..
        } = effect
        {
            if effect_identity != agent_identity {
                continue;
            }
            if authorized.replace(peer_endpoint.clone()).is_some() {
                return Err(MobError::WiringError(format!(
                    "{context} produced duplicate generated member peer rebind authority for '{}'",
                    agent_identity.0
                )));
            }
        }
    }
    authorized.ok_or_else(|| {
        MobError::WiringError(format!(
            "{context} produced no generated member peer rebind authority for '{}'",
            agent_identity.0
        ))
    })
}

fn authorize_seeded_member_peer_rebind(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    agent_identity: &crate::ids::AgentIdentity,
    context: &'static str,
) -> Result<TrustedPeerDescriptor, MobError> {
    let dsl_identity = crate::machines::mob_machine::AgentIdentity::from_domain(agent_identity);
    let expected_peer_endpoint = authority
        .state()
        .member_peer_endpoints
        .get(&dsl_identity)
        .cloned()
        .ok_or_else(|| {
            MobError::WiringError(format!(
                "{context}: peer-only rebind for '{agent_identity}' requires MobMachine member peer endpoint authority"
            ))
        })?;
    let transition = apply_seeded_mob_input_collect_transition(
        authority,
        crate::machines::mob_machine::MobMachineInput::AuthorizeMemberPeerRebind {
            agent_identity: dsl_identity.clone(),
            expected_peer_endpoint,
        },
        context,
    )?;
    let endpoint =
        member_peer_rebind_endpoint_from_transition(&transition, &dsl_identity, context)?;
    trusted_peer_descriptor_from_dsl_member_endpoint(&endpoint)
}

async fn apply_resume_peer_only_rebind_authority(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    roster: &mut Roster,
    runtime_metadata: &Arc<dyn crate::store::MobRuntimeMetadataStore>,
    mob_id: &MobId,
    agent_identity: &crate::ids::AgentIdentity,
    rebind_observation: &super::provisioner::PeerOnlyRebindObservation,
    context: &'static str,
) -> Result<(MemberRef, super::provisioner::PeerOnlyRebindAuthority), MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let authorized_peer = authorize_seeded_member_peer_rebind(authority, agent_identity, context)?;
    if authorized_peer.name != rebind_observation.observed_peer.name
        || authorized_peer.peer_id != rebind_observation.observed_peer.peer_id
        || authorized_peer.address != rebind_observation.observed_peer.address
        || authorized_peer.pubkey != rebind_observation.observed_peer.pubkey
    {
        return Err(MobError::WiringError(format!(
            "resume peer-only rebind for '{agent_identity}' observed endpoint outside generated MobMachine authority"
        )));
    }
    let rebind_authority = super::provisioner::PeerOnlyRebindAuthority {
        peer: authorized_peer.clone(),
        bootstrap_token: rebind_observation.bootstrap_token.clone(),
    };
    let identities = std::collections::BTreeSet::from([agent_identity.clone()]);
    let peer_id = authorized_peer.peer_id.to_string();
    let address = authorized_peer.address.to_string();
    let updated_entries = roster.replace_backend_peer_binding_for_identities(
        &identities,
        &peer_id,
        &address,
        Some(rebind_observation.bootstrap_token.clone()),
    );
    if updated_entries.is_empty() {
        return Err(MobError::WiringError(format!(
            "resume rebound peer binding for '{agent_identity}' requires roster projection for MobMachine member peer authority"
        )));
    }
    for (identity, generation, pubkey) in updated_entries {
        runtime_metadata
            .upsert_external_binding_overlay(
                mob_id,
                &crate::store::ExternalBindingOverlayRecord {
                    agent_identity: identity,
                    generation,
                    normalized_member_ref: Some(MemberRef::BackendPeer {
                        peer_id: peer_id.clone(),
                        address: address.clone(),
                        pubkey,
                        bootstrap_token: None,
                        session_id: None,
                    }),
                    bootstrap_token: Some(rebind_observation.bootstrap_token.clone()),
                    status: crate::store::ExternalBindingOverlayStatus::Normalized,
                    updated_at: chrono::Utc::now(),
                },
            )
            .await?;
    }

    // Row #314: record the machine-owned external-member rebind capability for
    // the peer-only resume rebind. A non-empty rebind token means the member is
    // supervisor-reboundable.
    let rebind_capability = if rebind_observation.bootstrap_token.is_empty() {
        mob_dsl::ExternalMemberRebindCapability::Unavailable
    } else {
        mob_dsl::ExternalMemberRebindCapability::Available
    };
    apply_seeded_mob_input(
        authority,
        mob_dsl::MobMachineInput::SetExternalMemberRebindCapability {
            agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
            capability: rebind_capability,
        },
        "resume_peer_only_set_external_member_rebind_capability",
    )?;

    roster
        .get_by_identity(agent_identity)
        .map(|entry| entry.member_ref.clone())
        .ok_or_else(|| {
            MobError::WiringError(format!(
                "resume rebound peer binding for '{agent_identity}' lost roster projection after MobMachine authority"
            ))
        })
        .map(|member_ref| (member_ref, rebind_authority))
}

fn apply_seeded_mob_input_collect_transition(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    input: crate::machines::mob_machine::MobMachineInput,
    context: &'static str,
) -> Result<crate::machines::mob_machine::MobMachineTransition, MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let input_debug = format!("{input:?}");
    let transition = mob_dsl::MobMachineMutator::apply(authority, input).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine seeded authority ({context}) rejected {input_debug}: {error}"
        ))
    })?;
    Ok(transition)
}

/// Map a typed wire bridge rejection cause onto the MobMachine observation
/// enum. The wire cause is `#[non_exhaustive]`; any future variant the mob does
/// not yet understand maps to `Internal`, which MobMachine classifies as
/// `FatalBubbleUp` — failing closed (no recovery) on an unrecognized cause.
fn seeded_mob_bridge_rejection_cause(
    cause: super::bridge_protocol::BridgeRejectionCause,
) -> crate::machines::mob_machine::MobBridgeRejectionCause {
    use super::bridge_protocol::BridgeRejectionCause as Wire;
    use crate::machines::mob_machine::MobBridgeRejectionCause as Mob;
    match cause {
        Wire::NotBound => Mob::NotBound,
        Wire::StaleSupervisor => Mob::StaleSupervisor,
        Wire::SenderMismatch => Mob::SenderMismatch,
        Wire::AlreadyBound => Mob::AlreadyBound,
        Wire::InvalidBootstrapToken => Mob::InvalidBootstrapToken,
        Wire::UnsupportedProtocolVersion => Mob::UnsupportedProtocolVersion,
        Wire::InvalidSupervisorSpec => Mob::InvalidSupervisorSpec,
        Wire::InvalidPeerSpec => Mob::InvalidPeerSpec,
        Wire::AddressMismatch => Mob::AddressMismatch,
        Wire::Unsupported => Mob::Unsupported,
        Wire::Internal => Mob::Internal,
        _ => Mob::Internal,
    }
}

/// Mirror MobMachine's bridge-rejection recovery verdict for a typed wire
/// rejection cause, using the resume builder's seeded `dsl_authority`.
///
/// The resume builder owns the MobMachine authority during reconciliation. When
/// `reconcile_peer_only_trust` returns a rejection observation, the builder —
/// not the provisioner — classifies the carried cause here. Returns `true` only
/// for `RebindRecover`; fails closed (returns an error) if the machine emits no
/// verdict.
fn classify_seeded_bridge_rejection_recovery(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    cause: super::bridge_protocol::BridgeRejectionCause,
    context: &'static str,
) -> Result<bool, MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let rejection_cause = seeded_mob_bridge_rejection_cause(cause);
    let transition = apply_seeded_mob_input_collect_transition(
        authority,
        mob_dsl::MobMachineInput::ClassifyBridgeRejectionRecovery { rejection_cause },
        context,
    )?;
    let (effect_cause, recovery) = transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::BridgeRejectionRecoveryClassified {
                rejection_cause,
                recovery,
            } => Some((*rejection_cause, *recovery)),
            _ => None,
        })
        .ok_or_else(|| {
            MobError::Internal(
                "MobMachine accepted bridge rejection cause but emitted no recovery verdict".into(),
            )
        })?;
    if effect_cause != rejection_cause {
        return Err(MobError::Internal(format!(
            "MobMachine bridge-rejection recovery drift: input={rejection_cause:?}, effect={effect_cause:?}"
        )));
    }
    Ok(matches!(
        recovery,
        mob_dsl::MobBridgeRejectionRecovery::RebindRecover
    ))
}

fn seeded_mob_topology_freshness_authority<P>(
    authority: &crate::machines::mob_machine::MobMachineAuthority,
    topology_epoch: &Arc<std::sync::atomic::AtomicU64>,
) -> P
where
    P: SeededMobTopologyFreshness,
{
    topology_epoch.store(
        authority.state().topology_epoch,
        std::sync::atomic::Ordering::Release,
    );
    P::from_live_seeded_topology_epoch(
        Arc::clone(topology_epoch),
        authority.generated_authority_owner_token(),
    )
}

trait SeededMobTopologyFreshness {
    fn from_live_seeded_topology_epoch(
        topology_epoch: Arc<std::sync::atomic::AtomicU64>,
        source_owner_token: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self;
}

impl SeededMobTopologyFreshness
    for crate::generated::protocol_mob_member_trust_wiring::MobTopologyFreshnessAuthority
{
    fn from_live_seeded_topology_epoch(
        topology_epoch: Arc<std::sync::atomic::AtomicU64>,
        source_owner_token: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self::from_live_topology_epoch(topology_epoch, source_owner_token)
    }
}

impl SeededMobTopologyFreshness
    for crate::generated::protocol_mob_member_trust_unwiring::MobTopologyFreshnessAuthority
{
    fn from_live_seeded_topology_epoch(
        topology_epoch: Arc<std::sync::atomic::AtomicU64>,
        source_owner_token: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self::from_live_topology_epoch(topology_epoch, source_owner_token)
    }
}

impl SeededMobTopologyFreshness
    for crate::generated::protocol_mob_member_peer_overlay::MobTopologyFreshnessAuthority
{
    fn from_live_seeded_topology_epoch(
        topology_epoch: Arc<std::sync::atomic::AtomicU64>,
        source_owner_token: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self::from_live_topology_epoch(topology_epoch, source_owner_token)
    }
}

impl SeededMobTopologyFreshness
    for crate::generated::protocol_mob_external_peer_trust_repair::MobTopologyFreshnessAuthority
{
    fn from_live_seeded_topology_epoch(
        topology_epoch: Arc<std::sync::atomic::AtomicU64>,
        source_owner_token: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self::from_live_topology_epoch(topology_epoch, source_owner_token)
    }
}

fn apply_seeded_mob_signal(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    signal: crate::machines::mob_machine::MobMachineSignal,
    context: &'static str,
) -> Result<(), MobError> {
    let _ = apply_seeded_mob_signal_transition(authority, signal, context)?;
    Ok(())
}

fn apply_seeded_mob_signal_transition(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    signal: crate::machines::mob_machine::MobMachineSignal,
    context: &'static str,
) -> Result<crate::machines::mob_machine::MobMachineTransition, MobError> {
    let signal_debug = format!("{signal:?}");
    let transition = authority.apply_signal(signal).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine seeded authority ({context}) rejected {signal_debug}: {error}"
        ))
    })?;
    Ok(transition)
}

fn seeded_effects_include_wiring_graph_change(
    effects: &[crate::machines::mob_machine::MobMachineEffect],
) -> bool {
    effects.iter().any(|effect| {
        matches!(
            effect,
            crate::machines::mob_machine::MobMachineEffect::WiringGraphChanged { .. }
        )
    })
}

fn resume_member_repair_authority_from_transition(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    topology_epoch: &Arc<std::sync::atomic::AtomicU64>,
    transition: &crate::machines::mob_machine::MobMachineTransition,
    edge: &crate::machines::mob_machine::WiringEdge,
    peer_id: &str,
    context: &'static str,
) -> Result<CommsTrustMutationAuthority, MobError> {
    let effects = transition.effects();
    let graph_changed = seeded_effects_include_wiring_graph_change(effects);
    let repair_requested = effects.iter().any(|effect| {
        matches!(
            effect,
            crate::machines::mob_machine::MobMachineEffect::WiringTrustRepairRequested {
                edge: effect_edge
            } if *effect_edge == *edge
        )
    });
    if !repair_requested || graph_changed {
        return Err(MobError::WiringError(format!(
            "{context} produced no generated member trust repair authority for peer '{peer_id}'"
        )));
    }

    let handoff_transition = apply_seeded_mob_input_collect_transition(
        authority,
        crate::machines::mob_machine::MobMachineInput::AuthorizeMemberTrustWiring {
            edge: edge.clone(),
            a_identity: edge.a.clone(),
            b_identity: edge.b.clone(),
        },
        context,
    )?;
    crate::generated::protocol_mob_member_trust_wiring::extract_obligations_with_freshness(
        &handoff_transition,
        seeded_mob_topology_freshness_authority(authority, topology_epoch),
    )
    .into_iter()
    .find_map(|obligation| {
        if obligation.edge() != edge
            || (obligation.a_peer_id().0 != peer_id && obligation.b_peer_id().0 != peer_id)
        {
            return None;
        }
        let identity = if obligation.a_peer_id().0 == peer_id {
            edge.a.0.as_str()
        } else {
            edge.b.0.as_str()
        };
        crate::generated::protocol_mob_member_trust_wiring::repair_authority_for_identity_with_live_authority(
            &obligation,
            identity,
            peer_id,
            authority,
        )
        .ok()
    })
    .ok_or_else(|| {
        MobError::WiringError(format!(
            "{context} produced no generated member trust handoff for peer '{peer_id}'"
        ))
    })
}

fn resume_external_repair_authority_from_transition(
    authority: &crate::machines::mob_machine::MobMachineAuthority,
    topology_epoch: &Arc<std::sync::atomic::AtomicU64>,
    transition: &crate::machines::mob_machine::MobMachineTransition,
    edge: &crate::machines::mob_machine::ExternalPeerEdge,
    peer_id: &str,
    context: &'static str,
) -> Result<CommsTrustMutationAuthority, MobError> {
    let effects = transition.effects();
    let graph_changed = seeded_effects_include_wiring_graph_change(effects);
    let repair_obligation =
        crate::generated::protocol_mob_external_peer_trust_repair::extract_obligations_with_freshness(
            transition,
            seeded_mob_topology_freshness_authority(authority, topology_epoch),
        )
        .into_iter()
        .find(|obligation| obligation.edge() == edge);
    if let Some(obligation) = repair_obligation
        && !graph_changed
    {
        return crate::generated::protocol_mob_external_peer_trust_repair::repair_authority_for_peer(
            &obligation,
            peer_id,
        )
        .map_err(MobError::WiringError);
    }
    Err(MobError::WiringError(format!(
        "{context} produced no generated external trust repair authority"
    )))
}

fn unexpected_resume_trust_mutation_result(
    operation: &'static str,
    result: CommsTrustMutationResult,
) -> SendError {
    SendError::Internal(format!(
        "{operation} returned unexpected trust mutation result: {result:?}"
    ))
}

async fn preflight_resume_trust_mutations(
    mutations: &[ResumeTrustMutation],
    mob_owner_token: &Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), SendError> {
    for mutation in mutations {
        if mutation.authority.is_mob_machine_source() {
            mutation
                .comms
                .validate_recovered_generated_mob_trust_owner(Arc::clone(mob_owner_token))
                .await?;
        }
        match &mutation.operation {
            ResumeTrustMutationOperation::Add(peer) => {
                mutation
                    .authority
                    .preflight_public_add(mutation.comms.peer_id(), peer)
                    .map_err(SendError::Validation)?;
                TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                    .map_err(SendError::Validation)?;
                let owned_rows = mutation
                    .comms
                    .trusted_peer_projection_snapshot_for_source(
                        mutation.authority.trust_row_owner_kind(),
                    )
                    .await
                    .map_err(|error| SendError::Unsupported(error.to_string()))?;
                if let Some(existing) = owned_rows
                    .iter()
                    .find(|existing| existing.peer_id == peer.peer_id)
                    && existing != peer
                {
                    return Err(SendError::Validation(format!(
                        "resume trust repair for peer {} would rewrite existing generated {:?} trust material",
                        peer.peer_id,
                        mutation.authority.trust_row_owner_kind()
                    )));
                }
            }
            ResumeTrustMutationOperation::Remove(peer_id) => {
                let parsed_peer_id = PeerId::parse(peer_id)
                    .map_err(|error| SendError::Validation(error.to_string()))?;
                mutation
                    .authority
                    .preflight_public_remove(mutation.comms.peer_id(), parsed_peer_id)
                    .map_err(SendError::Validation)?;
            }
        }
    }
    Ok(())
}

async fn bind_resume_trust_mutation_owners(
    mutations: &[ResumeTrustMutation],
    mob_owner_token: &Arc<dyn std::any::Any + Send + Sync>,
) -> Result<(), SendError> {
    for mutation in mutations {
        if mutation.authority.is_mob_machine_source() {
            mutation
                .comms
                .install_recovered_generated_mob_trust_owner(Arc::clone(mob_owner_token))
                .await?;
        }
    }
    Ok(())
}

async fn apply_resume_trust_mutation(mutation: ResumeTrustMutation) -> Result<(), SendError> {
    match mutation.operation {
        ResumeTrustMutationOperation::Add(peer) => match mutation
            .comms
            .apply_trust_mutation(CommsTrustMutation::AddTrustedPeer {
                peer,
                authority: mutation.authority,
            })
            .await?
        {
            CommsTrustMutationResult::Added { .. } => Ok(()),
            result => Err(unexpected_resume_trust_mutation_result(
                "resume add trusted peer",
                result,
            )),
        },
        ResumeTrustMutationOperation::Remove(peer_id) => match mutation
            .comms
            .apply_trust_mutation(CommsTrustMutation::RemoveTrustedPeer {
                peer_id,
                authority: mutation.authority,
            })
            .await?
        {
            CommsTrustMutationResult::Removed { .. } => Ok(()),
            result => Err(unexpected_resume_trust_mutation_result(
                "resume remove trusted peer",
                result,
            )),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn resume_member_observed_cleanup_authority(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    topology_epoch: &Arc<std::sync::atomic::AtomicU64>,
    edge: &crate::machines::mob_machine::WiringEdge,
    local_identity: &crate::ids::AgentIdentity,
    local_peer_id: &PeerId,
    peer_identity: &crate::ids::AgentIdentity,
    peer_id: &PeerId,
    context: &'static str,
) -> Result<CommsTrustMutationAuthority, MobError> {
    let local_dsl = crate::machines::mob_machine::AgentIdentity::from_domain(local_identity);
    let peer_dsl = crate::machines::mob_machine::AgentIdentity::from_domain(peer_identity);
    let (a_peer_id, b_peer_id) = if edge.a == local_dsl && edge.b == peer_dsl {
        (
            crate::machines::mob_machine::PeerId::from(local_peer_id.to_string()),
            crate::machines::mob_machine::PeerId::from(peer_id.to_string()),
        )
    } else if edge.a == peer_dsl && edge.b == local_dsl {
        (
            crate::machines::mob_machine::PeerId::from(peer_id.to_string()),
            crate::machines::mob_machine::PeerId::from(local_peer_id.to_string()),
        )
    } else {
        return Err(MobError::WiringError(format!(
            "{context} edge does not connect '{local_identity}' and '{peer_identity}'"
        )));
    };
    let transition = apply_seeded_mob_input_collect_transition(
        authority,
        crate::machines::mob_machine::MobMachineInput::AuthorizeMemberTrustCleanupObserved {
            edge: edge.clone(),
            a_identity: edge.a.clone(),
            a_peer_id,
            b_identity: edge.b.clone(),
            b_peer_id,
        },
        context,
    )?;
    crate::generated::protocol_mob_member_trust_unwiring::extract_obligations_with_freshness(
        &transition,
        seeded_mob_topology_freshness_authority(authority, topology_epoch),
    )
    .into_iter()
    .find_map(|obligation| {
        if obligation.edge() != edge {
            return None;
        }
        crate::generated::protocol_mob_member_trust_unwiring::unwiring_authority_for_identity(
            &obligation,
            peer_identity.as_str(),
            &peer_id.to_string(),
        )
        .ok()
    })
    .ok_or_else(|| {
        MobError::WiringError(format!(
            "{context} produced no generated observed cleanup authority for peer '{peer_id}'"
        ))
    })
}

fn peer_only_trust_overlay_from_mob_machine(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    topology_epoch: &Arc<std::sync::atomic::AtomicU64>,
    agent_identity: &crate::ids::AgentIdentity,
    context: &'static str,
) -> Result<super::provisioner::PeerOnlyTrustOverlay, MobError> {
    let dsl_identity = crate::machines::mob_machine::AgentIdentity::from_domain(agent_identity);
    let expected_peer_endpoint = authority
        .state()
        .member_peer_endpoints
        .get(&dsl_identity)
        .cloned()
        .ok_or_else(|| {
            MobError::WiringError(format!(
                "{context}: peer-only trust overlay for '{agent_identity}' requires MobMachine member peer endpoint authority"
            ))
        })?;
    let transition = apply_seeded_mob_input_collect_transition(
        authority,
        crate::machines::mob_machine::MobMachineInput::AuthorizeMemberPeerOverlay {
            agent_identity: dsl_identity.clone(),
            expected_peer_endpoint,
        },
        context,
    )?;
    let obligation =
        crate::generated::protocol_mob_member_peer_overlay::extract_obligations_with_freshness(
            &transition,
            seeded_mob_topology_freshness_authority(authority, topology_epoch),
        )
        .into_iter()
        .find(|obligation| obligation.agent_identity() == &dsl_identity)
        .ok_or_else(|| {
            MobError::WiringError(format!(
                "{context} produced no generated MobMachine peer overlay handoff for '{agent_identity}'"
            ))
    })?;
    super::provisioner::PeerOnlyTrustOverlay::from_generated_mob_member_peer_overlay(&obligation)
}

fn peer_only_member_has_pending_supervisor_operation(
    pending_peer_ids: &std::collections::BTreeSet<String>,
    member_ref: &MemberRef,
) -> bool {
    matches!(
        member_ref,
        MemberRef::BackendPeer {
            peer_id,
            session_id: None,
            ..
        } if pending_peer_ids.contains(peer_id)
    )
}

fn apply_seeded_member_session_binding(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    agent_identity: &crate::ids::AgentIdentity,
    agent_runtime_id: &crate::ids::AgentRuntimeId,
    bridge_session_id: &meerkat_core::types::SessionId,
    context: &'static str,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
    let replacing = authority
        .state()
        .member_session_bindings
        .get(&dsl_identity)
        .cloned();
    apply_seeded_mob_signal(
        authority,
        mob_dsl::MobMachineSignal::RecoverMemberSessionBinding {
            agent_identity: dsl_identity,
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
            bridge_session_id: mob_dsl::SessionId::from_domain(bridge_session_id),
            replacing,
        },
        context,
    )
}

async fn latest_persisted_session_for_member(
    session_service: &dyn MobSessionService,
    listed_sessions: &[meerkat_core::service::SessionSummary],
    missing_session_id: &meerkat_core::types::SessionId,
    mob_id: &crate::ids::MobId,
    role: &crate::ids::ProfileName,
    agent_identity: &crate::ids::AgentIdentity,
) -> Result<Option<(meerkat_core::types::SessionId, meerkat_core::Session)>, MobError> {
    let canonical_comms_name = super::actor::render_member_comms_name(
        mob_id.as_str(),
        role.as_str(),
        agent_identity.as_str(),
    )?;

    // Candidate matching runs over the metadata-only read seam: the member
    // identity facts (typed binding, comms name) live on session metadata, so
    // scanning the realm never materializes full transcripts.
    let mut best: Option<(
        meerkat_core::time_compat::SystemTime,
        meerkat_core::types::SessionId,
    )> = None;

    for summary in listed_sessions {
        if &summary.session_id == missing_session_id {
            continue;
        }
        let Some(view) = session_service
            .load_persisted_session_metadata(&summary.session_id)
            .await?
        else {
            continue;
        };
        if !persisted_session_matches_member(
            view.session_metadata.as_ref(),
            mob_id,
            role,
            agent_identity,
            &canonical_comms_name,
        ) {
            continue;
        }
        let replace = best
            .as_ref()
            .is_none_or(|(updated_at, _)| summary.updated_at > *updated_at);
        if replace {
            best = Some((summary.updated_at, summary.session_id.clone()));
        }
    }

    let Some((_, winner_session_id)) = best else {
        return Ok(None);
    };
    // Full-load ONLY the winner. A winner that vanishes between the metadata
    // match and the full load reads as "no replacement" (`Ok(None)`) — the
    // caller records the member's restore failure exactly as if no candidate
    // had matched.
    Ok(session_service
        .load_persisted_session(&winner_session_id)
        .await?
        .map(|session| (winner_session_id, session)))
}

fn persisted_session_matches_member(
    metadata: Option<&meerkat_core::SessionMetadata>,
    mob_id: &crate::ids::MobId,
    role: &crate::ids::ProfileName,
    agent_identity: &crate::ids::AgentIdentity,
    canonical_comms_name: &str,
) -> bool {
    let Some(metadata) = metadata else {
        return false;
    };
    if metadata.mob_member_binding.as_ref().is_some_and(|binding| {
        binding.mob_id == mob_id.as_str()
            && binding.role == role.as_str()
            && binding.member == agent_identity.as_str()
    }) {
        return true;
    }
    metadata.comms_name.as_deref().is_some_and(|comms_name| {
        crate::build::resumed_comms_name_matches_current_or_legacy(canonical_comms_name, comms_name)
    })
}

fn authorize_seeded_session_provision_operation_owner(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    agent_identity: &crate::ids::AgentIdentity,
    agent_runtime_id: &crate::ids::AgentRuntimeId,
    bridge_session_id: &meerkat_core::types::SessionId,
    context: &'static str,
) -> Result<meerkat_core::types::SessionId, MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
    let dsl_session_id = mob_dsl::SessionId::from_domain(bridge_session_id);
    let replacing = authority
        .state()
        .member_session_bindings
        .get(&dsl_identity)
        .cloned();
    let transition = apply_seeded_mob_signal_transition(
        authority,
        mob_dsl::MobMachineSignal::RecoverMemberSessionBinding {
            agent_identity: dsl_identity.clone(),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
            bridge_session_id: dsl_session_id.clone(),
            replacing,
        },
        context,
    )?;
    let authorized = transition.effects().iter().any(|effect| {
        matches!(
            effect,
            mob_dsl::MobMachineEffect::SessionProvisionOperationOwnerAuthorized {
                agent_identity: effect_identity,
                session_id,
            } if effect_identity == &dsl_identity && session_id == &dsl_session_id
        )
    });
    if !authorized {
        return Err(MobError::Internal(format!(
            "MobMachine seeded authority ({context}) produced no session provision operation owner for '{agent_identity}'"
        )));
    }
    Ok(bridge_session_id.clone())
}

#[cfg(feature = "runtime-adapter")]
async fn bind_seeded_session_operation_owner_context(
    provisioner: &dyn super::provisioner::MobProvisioner,
    runtime_adapter: &meerkat_runtime::MeerkatMachine,
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    entry: &RosterEntry,
    bridge_session_id: &meerkat_core::types::SessionId,
    context: &'static str,
) -> Result<(), MobError> {
    let generated_owner_session_id = authorize_seeded_session_provision_operation_owner(
        authority,
        &entry.agent_identity,
        &entry.agent_runtime_id,
        bridge_session_id,
        context,
    )?;
    if &generated_owner_session_id != bridge_session_id {
        return Err(MobError::Internal(format!(
            "MobMachine seeded authority ({context}) authorized operation owner '{generated_owner_session_id}' for '{}' but restore bridge session is '{bridge_session_id}'",
            entry.agent_identity
        )));
    }
    let bindings = runtime_adapter
        .prepare_local_session_bindings(generated_owner_session_id.clone())
        .await
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to prepare MeerkatMachine restore operation bindings for '{}': {error}",
                entry.agent_identity
            ))
        })?;
    if bindings.session_id() != bridge_session_id {
        return Err(MobError::Internal(format!(
            "MeerkatMachine restore operation bindings for '{}' returned session '{}' but expected '{bridge_session_id}'",
            entry.agent_identity,
            bindings.session_id()
        )));
    }
    if !meerkat_runtime::session_runtime_bindings_have_machine_authority(&bindings) {
        return Err(MobError::Internal(format!(
            "MeerkatMachine restore operation bindings for '{}' lacked machine authority",
            entry.agent_identity
        )));
    }
    provisioner
        .bind_member_owner_context(
            &entry.member_ref,
            generated_owner_session_id,
            Arc::clone(bindings.ops_lifecycle()),
        )
        .await
}

#[allow(clippy::too_many_arguments)]
fn apply_seeded_member_addressability(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    agent_identity: &crate::ids::AgentIdentity,
    agent_runtime_id: &crate::ids::AgentRuntimeId,
    fence_token: crate::ids::FenceToken,
    profile_name: &crate::ids::ProfileName,
    runtime_mode: crate::MobRuntimeMode,
    external_addressable: bool,
    context: &'static str,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;

    apply_seeded_mob_signal(
        authority,
        mob_dsl::MobMachineSignal::RecoverRosterMember {
            agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
            fence_token: mob_dsl::FenceToken::from_domain(fence_token),
            generation: mob_dsl::Generation::from_domain(agent_runtime_id.generation),
            profile_name: profile_name.as_str().to_string(),
            runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::from(runtime_mode),
            external_addressable,
        },
        context,
    )
}

fn finish_seeded_mob_authority_phase(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    target_phase: MobState,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;

    match target_phase {
        MobState::Creating | MobState::Running => Ok(()),
        MobState::Stopped => {
            apply_seeded_mob_input(authority, mob_dsl::MobMachineInput::Stop, "seed_phase_stop")
        }
        MobState::Completed => apply_seeded_mob_input(
            authority,
            mob_dsl::MobMachineInput::Complete,
            "seed_phase_complete",
        ),
        MobState::Destroyed => apply_seeded_mob_input(
            authority,
            mob_dsl::MobMachineInput::Destroy,
            "seed_phase_destroy",
        ),
    }
}

fn seed_mob_definition_spawn_policy(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    definition: &MobDefinition,
    context: &'static str,
) -> Result<(), MobError> {
    if !super::spawn_policy::definition_config_has_policy_fact(definition.spawn_policy.as_ref()) {
        return Ok(());
    }

    apply_seeded_mob_input(
        authority,
        crate::machines::mob_machine::MobMachineInput::SetSpawnPolicy {
            enabled: super::spawn_policy::definition_config_enables_policy(
                definition.spawn_policy.as_ref(),
            ),
        },
        context,
    )
}

fn seeded_mob_public_phase(state: &crate::machines::mob_machine::MobMachineState) -> MobState {
    use crate::machines::mob_machine::MobPhase;

    if state.destroy_admitted {
        return MobState::Destroyed;
    }
    match state.lifecycle_phase {
        MobPhase::Running => MobState::Running,
        MobPhase::Stopped => MobState::Stopped,
        MobPhase::Completed => MobState::Completed,
        MobPhase::Destroyed => MobState::Destroyed,
    }
}

#[cfg(feature = "runtime-adapter")]
fn canonical_runtime_adapter_for_session_service(
    session_service: &Arc<dyn MobSessionService>,
    runtime_adapter: RuntimeAdapterOption,
) -> Result<RuntimeAdapterOption, MobError> {
    let service_adapter = session_service.runtime_adapter();
    match (runtime_adapter, service_adapter) {
        (Some(adapter), Some(service_adapter))
            if !adapter.shares_runtime_persistence_with(&service_adapter) =>
        {
            Err(MobError::Internal(
                "explicit mob runtime adapter does not share the session service runtime persistence authority".to_string(),
            ))
        }
        (Some(adapter), _) => Ok(Some(adapter)),
        (None, service_adapter) => Ok(service_adapter),
    }
}

fn inline_external_addressable(definition: &MobDefinition, role: &ProfileName) -> bool {
    definition
        .profiles
        .get(role)
        .and_then(|binding| binding.as_inline())
        .map(|profile| profile.external_addressable)
        .unwrap_or(false)
}

fn dsl_external_peer_key(
    local: &AgentIdentity,
    peer_name: &meerkat_core::comms::PeerName,
) -> crate::machines::mob_machine::ExternalPeerKey {
    crate::machines::mob_machine::ExternalPeerKey::new(
        crate::machines::mob_machine::AgentIdentity::from_domain(local),
        crate::machines::mob_machine::PeerName(peer_name.as_str().to_owned()),
    )
}

fn dsl_wiring_edge(
    a: &AgentIdentity,
    b: &AgentIdentity,
) -> crate::machines::mob_machine::WiringEdge {
    let dsl_a = crate::machines::mob_machine::AgentIdentity::from_domain(a);
    let dsl_b = crate::machines::mob_machine::AgentIdentity::from_domain(b);
    crate::machines::mob_machine::WiringEdge::new(dsl_a, dsl_b)
}

fn dsl_kickoff_phase(
    phase: crate::roster::MobMemberKickoffPhase,
) -> crate::machines::mob_machine::KickoffPhase {
    match phase {
        crate::roster::MobMemberKickoffPhase::Pending => {
            crate::machines::mob_machine::KickoffPhase::Pending
        }
        crate::roster::MobMemberKickoffPhase::Starting => {
            crate::machines::mob_machine::KickoffPhase::Starting
        }
        crate::roster::MobMemberKickoffPhase::Started => {
            crate::machines::mob_machine::KickoffPhase::Started
        }
        crate::roster::MobMemberKickoffPhase::CallbackPending => {
            crate::machines::mob_machine::KickoffPhase::CallbackPending
        }
        crate::roster::MobMemberKickoffPhase::Failed => {
            crate::machines::mob_machine::KickoffPhase::Failed
        }
        crate::roster::MobMemberKickoffPhase::Cancelled => {
            crate::machines::mob_machine::KickoffPhase::Cancelled
        }
    }
}

fn trusted_peer_descriptor_from_dsl_external_endpoint(
    endpoint: &crate::machines::mob_machine::ExternalPeerEndpoint,
) -> Result<TrustedPeerDescriptor, MobError> {
    TrustedPeerDescriptor::unsigned_with_pubkey(
        endpoint.name.0.clone(),
        endpoint.peer_id.0.clone(),
        endpoint.signing_key.0,
        endpoint.address.0.clone(),
    )
    .map_err(|error| {
        MobError::WiringError(format!(
            "invalid recovered external peer endpoint from MobMachine authority: {error}"
        ))
    })
}

fn trusted_peer_descriptor_from_dsl_member_endpoint(
    endpoint: &crate::machines::mob_machine::MemberPeerEndpoint,
) -> Result<TrustedPeerDescriptor, MobError> {
    TrustedPeerDescriptor::unsigned_with_pubkey(
        endpoint.name.0.clone(),
        endpoint.peer_id.0.clone(),
        endpoint.signing_key.0,
        endpoint.address.0.clone(),
    )
    .map_err(|error| {
        MobError::WiringError(format!(
            "invalid recovered member peer endpoint from MobMachine authority: {error}"
        ))
    })
}

fn recover_owner_bridge_session_authority(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    bridge_session_id: &SessionId,
    destroy_on_owner_archive: bool,
    implicit_delegation_mob: bool,
    context: &'static str,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;

    apply_seeded_mob_signal(
        authority,
        mob_dsl::MobMachineSignal::RecoverOwnerBridgeSession {
            bridge_session_id: mob_dsl::SessionId::from_domain(bridge_session_id),
            destroy_on_owner_archive,
            implicit_delegation_mob,
        },
        context,
    )
}

fn recover_owner_bridge_session_authority_from_history(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    events: &[crate::event::MobEvent],
) -> Result<(), MobError> {
    for event in events {
        if let MobEventKind::MobOwnerBridgeSessionBound {
            bridge_session_id,
            destroy_on_owner_archive,
            implicit_delegation_mob,
        } = &event.kind
        {
            recover_owner_bridge_session_authority(
                authority,
                bridge_session_id,
                *destroy_on_owner_archive,
                *implicit_delegation_mob,
                "recover_owner_bridge_session_history",
            )?;
        }
    }
    Ok(())
}

/// Replay typed durable mob events as recovery inputs to generated MobMachine
/// authority. `Roster` remains a read model and is not used as the source of
/// machine facts.
fn seed_mob_authority_sync_from_events(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    events: &[crate::event::MobEvent],
    definition: &MobDefinition,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;

    for event in events {
        match &event.kind {
            MobEventKind::MobCreated { .. } => {}
            MobEventKind::MobOwnerBridgeSessionBound {
                bridge_session_id,
                destroy_on_owner_archive,
                implicit_delegation_mob,
            } => {
                recover_owner_bridge_session_authority(
                    authority,
                    bridge_session_id,
                    *destroy_on_owner_archive,
                    *implicit_delegation_mob,
                    "recover_owner_bridge_session",
                )?;
            }
            MobEventKind::MobCompleted => {
                apply_seeded_mob_input(
                    authority,
                    mob_dsl::MobMachineInput::Complete,
                    "recover_mob_completed",
                )?;
            }
            MobEventKind::MobDestroying => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::AdmitDestroyCleanup,
                    "recover_destroy_admitted",
                )?;
            }
            MobEventKind::MobDestroyStorageFinalizing => {
                if !authority.state().destroy_admitted {
                    apply_seeded_mob_signal(
                        authority,
                        mob_dsl::MobMachineSignal::AdmitDestroyCleanup,
                        "recover_destroy_finalizing_admission",
                    )?;
                }
                apply_seeded_mob_input(
                    authority,
                    mob_dsl::MobMachineInput::Destroy,
                    "recover_destroy_storage_finalizing",
                )?;
            }
            MobEventKind::MobReset => {}
            MobEventKind::MemberSpawned(member_spawned) => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRosterMember {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(
                            &member_spawned.agent_identity,
                        ),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                            &member_spawned.agent_runtime_id,
                        ),
                        fence_token: mob_dsl::FenceToken::from_domain(member_spawned.fence_token),
                        generation: mob_dsl::Generation::from_domain(
                            member_spawned.agent_runtime_id.generation,
                        ),
                        profile_name: member_spawned.role.as_str().to_string(),
                        runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::from(
                            member_spawned.runtime_mode,
                        ),
                        external_addressable: inline_external_addressable(
                            definition,
                            &member_spawned.role,
                        ),
                    },
                    "recover_member_spawned",
                )?;
                if let Some(bridge_session_id) = member_spawned
                    .bridge_member_ref
                    .as_ref()
                    .and_then(crate::event::MemberRef::bridge_session_id)
                {
                    apply_seeded_member_session_binding(
                        authority,
                        &member_spawned.agent_identity,
                        &member_spawned.agent_runtime_id,
                        bridge_session_id,
                        "recover_member_session_binding",
                    )?;
                }
                // Row #314: recover the machine-owned external-member rebind
                // capability from the persisted bridge_member_ref bootstrap
                // proof (preserved by `sanitized_member_ref`).
                let rebind_capability = member_spawned
                    .bridge_member_ref
                    .as_ref()
                    .map(super::actor::external_member_rebind_capability_from_member_ref)
                    .unwrap_or(mob_dsl::ExternalMemberRebindCapability::Unavailable);
                apply_seeded_mob_input(
                    authority,
                    mob_dsl::MobMachineInput::SetExternalMemberRebindCapability {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(
                            &member_spawned.agent_identity,
                        ),
                        capability: rebind_capability,
                    },
                    "recover_member_external_rebind_capability",
                )?;
            }
            MobEventKind::MemberReset {
                agent_identity,
                previous_generation,
                fence_token,
                agent_runtime_id,
                ..
            } => {
                let previous_agent_runtime_id =
                    AgentRuntimeId::new(agent_identity.clone(), *previous_generation);
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRosterMemberReset {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        previous_agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                            &previous_agent_runtime_id,
                        ),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
                        fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
                        generation: mob_dsl::Generation::from_domain(agent_runtime_id.generation),
                    },
                    "recover_member_reset",
                )?;
            }
            MobEventKind::MemberSessionBindingRecovered(recovered) => {
                let Some(bridge_session_id) = recovered.bridge_session_id() else {
                    return Err(MobError::Internal(format!(
                        "stored member-session binding recovery event for '{}' is missing bridge_session_id",
                        recovered.agent_identity
                    )));
                };
                apply_seeded_member_session_binding(
                    authority,
                    &recovered.agent_identity,
                    &recovered.agent_runtime_id,
                    bridge_session_id,
                    "recover_member_session_binding_recovered",
                )?;
            }
            MobEventKind::MemberRetirementStarted {
                agent_identity,
                agent_runtime_id,
                generation,
                releasing,
                session_id,
                retiring_peer_endpoint,
                ..
            } => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRosterMemberRetirementStarted {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
                        generation: mob_dsl::Generation::from_domain(*generation),
                        releasing: releasing.as_ref().map(mob_dsl::SessionId::from_domain),
                        session_id: session_id.as_ref().map(mob_dsl::SessionId::from_domain),
                        retiring_peer_endpoint: retiring_peer_endpoint
                            .as_ref()
                            .map(mob_dsl::MemberPeerEndpoint::from),
                    },
                    "recover_member_retirement_started",
                )?;
            }
            MobEventKind::RemoteMemberRuntimeRetired {
                agent_identity,
                agent_runtime_id,
                fence_token,
                generation,
            } => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRemoteMemberRuntimeRetired {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
                        fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
                        generation: mob_dsl::Generation::from_domain(*generation),
                    },
                    "recover_remote_member_runtime_retired",
                )?;
            }
            MobEventKind::RemoteMemberSupervisorRevoked {
                agent_identity,
                agent_runtime_id,
                fence_token,
                generation,
            } => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRemoteMemberSupervisorRevoked {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
                        fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
                        generation: mob_dsl::Generation::from_domain(*generation),
                    },
                    "recover_remote_member_supervisor_revoked",
                )?;
            }
            MobEventKind::MemberRetired {
                agent_identity,
                generation,
                ..
            } => {
                let agent_runtime_id = AgentRuntimeId::new(agent_identity.clone(), *generation);
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRosterMemberRetired {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&agent_runtime_id),
                    },
                    "recover_member_retired",
                )?;
            }
            MobEventKind::MemberKickoffUpdated { member, kickoff } => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverMemberKickoff {
                        member_id: mob_dsl::AgentIdentity::from_domain(member),
                        phase: dsl_kickoff_phase(kickoff.phase),
                        error: kickoff.error.clone(),
                    },
                    "recover_member_kickoff",
                )?;
            }
            MobEventKind::MembersWired { a, b } => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRosterWiring {
                        edge: dsl_wiring_edge(a, b),
                    },
                    "recover_members_wired",
                )?;
            }
            MobEventKind::MembersWiredBatch { edges } => {
                for edge in edges {
                    apply_seeded_mob_signal(
                        authority,
                        mob_dsl::MobMachineSignal::RecoverRosterWiring {
                            edge: dsl_wiring_edge(&edge.a, &edge.b),
                        },
                        "recover_members_wired_batch",
                    )?;
                }
            }
            MobEventKind::MembersUnwired { a, b } => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRosterUnwire {
                        edge: dsl_wiring_edge(a, b),
                    },
                    "recover_members_unwired",
                )?;
            }
            MobEventKind::ExternalPeerWired { local, spec } => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverExternalPeerWiring {
                        key: dsl_external_peer_key(local, &spec.name),
                        edge: mob_dsl::ExternalPeerEdge::new(
                            mob_dsl::AgentIdentity::from_domain(local),
                            mob_dsl::ExternalPeerEndpoint::from(spec),
                        ),
                    },
                    "recover_external_peer_wired",
                )?;
            }
            MobEventKind::ExternalPeerUnwired { local, peer_name } => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverExternalPeerUnwire {
                        key: dsl_external_peer_key(local, peer_name),
                    },
                    "recover_external_peer_unwired",
                )?;
            }
            _ => {}
        }
    }
    let recovered_wiring_edges = authority
        .state()
        .wiring_edges
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    for edge in recovered_wiring_edges {
        apply_seeded_mob_signal(
            authority,
            mob_dsl::MobMachineSignal::ConvergeRecoveredRosterTopology {
                a_identity: edge.a.clone(),
                b_identity: edge.b.clone(),
                edge,
            },
            "converge_recovered_roster_topology",
        )?;
    }
    seed_mob_definition_spawn_policy(authority, definition, "recover_definition_spawn_policy")?;
    Ok(())
}

async fn seed_mob_authority_sync_from_flow_runs(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    event_store: Arc<dyn crate::store::MobEventStore>,
    mob_id: &MobId,
) -> Result<(), MobError> {
    use crate::run::MobRun;

    let run_store = authority_validating_mob_run_store(run_store);
    let terminalization = super::terminalization::FlowTerminalizationAuthority::new(
        run_store.clone(),
        event_store,
        mob_id.clone(),
    );
    let mut runs = run_store.list_runs(mob_id, None).await?;
    runs.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.run_id.to_string().cmp(&right.run_id.to_string()))
    });

    for mut run in runs {
        super::recovery::reconcile_run_state(&mut run).map_err(|error| {
            MobError::Internal(format!("cannot resume flow run '{}': {error}", run.run_id))
        })?;
        if crate::run::mob_machine_run_status_is_terminal(&run.run_id, &run.status)?
            || run.flow_authority_inputs.is_empty()
        {
            continue;
        }

        MobRun::replay_flow_authority_inputs_into(
            authority,
            &run.flow_authority_inputs,
            &format!("resume_flow_run_{}", run.run_id),
        )?;
        if flow_run_replayed_active_admission(&run) {
            converge_recovered_active_flow_run(
                authority,
                run_store.clone(),
                &terminalization,
                run.run_id.clone(),
            )
            .await?;
        }
    }
    Ok(())
}

fn flow_run_replayed_active_admission(run: &crate::run::MobRun) -> bool {
    run.flow_authority_inputs
        .iter()
        .any(|record| matches!(record, crate::run::FlowAuthorityInputRecord::RunFlow(_)))
}

async fn converge_recovered_active_flow_run(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    terminalization: &super::terminalization::FlowTerminalizationAuthority,
    run_id: crate::ids::RunId,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;
    use crate::run::{MobMachineFlowRunCommand, MobRunStatus, flow_run};

    start_recovered_flow_run_if_pending(authority, run_store.clone(), &run_id).await?;
    cancel_recovered_unfinished_steps(authority, run_store.clone(), &run_id).await?;

    let before_terminal = run_store
        .get_run(&run_id)
        .await?
        .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
    if !crate::run::mob_machine_run_status_is_terminal(
        &before_terminal.run_id,
        &before_terminal.status,
    )? {
        // Terminal status and terminal event commit through the store's
        // combined seam (single SQLite transaction on durable storage) so
        // recovery cannot split terminal truth either.
        let _ = commit_recovered_flow_run_command(
            authority,
            run_store.clone(),
            &run_id,
            MobMachineFlowRunCommand::TerminalizeCanceled(flow_run::inputs::TerminalizeCanceled {}),
            Some(MobRunStatus::Canceled),
            Some((
                terminalization,
                super::terminalization::TerminalizationTarget::Canceled,
                before_terminal.flow_id.clone(),
            )),
            "resume_recovered_flow_terminalize_canceled",
        )
        .await?;
    }

    apply_recovered_flow_signal(
        authority,
        mob_dsl::MobMachineSignal::CompleteFlow,
        "resume_recovered_flow_complete",
    )?;
    apply_recovered_flow_signal(
        authority,
        mob_dsl::MobMachineSignal::FinishRun,
        "resume_recovered_flow_finish",
    )?;
    Ok(())
}

async fn start_recovered_flow_run_if_pending(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    run_id: &crate::ids::RunId,
) -> Result<(), MobError> {
    use crate::run::{MobMachineFlowRunCommand, MobRunStatus, flow_run};

    let run = run_store
        .get_run(run_id)
        .await?
        .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
    if run.status != MobRunStatus::Pending {
        return Ok(());
    }
    let _ = commit_recovered_flow_run_command(
        authority,
        run_store,
        run_id,
        MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {}),
        Some(MobRunStatus::Running),
        None,
        "resume_recovered_flow_start",
    )
    .await?;
    Ok(())
}

async fn cancel_recovered_unfinished_steps(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    run_id: &crate::ids::RunId,
) -> Result<(), MobError> {
    use crate::run::{MobMachineFlowRunCommand, flow_run};

    let run = run_store
        .get_run(run_id)
        .await?
        .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
    for step_id in run.ordered_steps()? {
        let is_terminal = run
            .flow_state
            .step_status
            .get(&step_id)
            .and_then(|status| *status)
            .is_some_and(|status| {
                matches!(
                    status,
                    flow_run::StepRunStatus::Completed
                        | flow_run::StepRunStatus::Failed
                        | flow_run::StepRunStatus::Skipped
                        | flow_run::StepRunStatus::Canceled
                )
            });
        if is_terminal {
            continue;
        }
        let _ = commit_recovered_flow_run_command(
            authority,
            run_store.clone(),
            run_id,
            MobMachineFlowRunCommand::CancelStep(flow_run::inputs::CancelStep { step_id }),
            None,
            None,
            "resume_recovered_flow_cancel_step",
        )
        .await?;
    }
    Ok(())
}

async fn commit_recovered_flow_run_command(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    run_id: &crate::ids::RunId,
    command: crate::run::MobMachineFlowRunCommand,
    next_status: Option<crate::run::MobRunStatus>,
    terminalization: Option<(
        &super::terminalization::FlowTerminalizationAuthority,
        super::terminalization::TerminalizationTarget,
        crate::ids::FlowId,
    )>,
    context: &'static str,
) -> Result<bool, MobError> {
    use crate::machines::mob_machine as mob_dsl;
    use crate::run::{MobMachineFlowAuthorityToken, apply_mob_machine_flow_run_command};

    let authority_input = command.authority_input(run_id);
    for attempt in 0..5u32 {
        let run = run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        if crate::run::mob_machine_run_status_is_terminal(&run.run_id, &run.status)? {
            return Ok(false);
        }

        let mut prepared = mob_dsl::MobMachineAuthority::recover_from_state(
            authority.state().clone(),
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "MobMachine recovered flow authority ({context}) could not recover state: {error}"
            ))
        })?;
        let input_debug = format!("{authority_input:?}");
        let transition =
            mob_dsl::MobMachineMutator::apply(&mut prepared, authority_input.clone()).map_err(
                |error| {
                    MobError::Internal(format!(
                        "MobMachine recovered flow authority ({context}) rejected {input_debug}: {error}"
                    ))
                },
            )?;
        let token =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        let outcome = apply_mob_machine_flow_run_command(
            &run.flow_state,
            prepared.state(),
            run_id,
            command.clone(),
            token,
            transition.effects(),
        )?;

        let won = match (&next_status, &terminalization) {
            (Some(_), Some((terminalization, target, flow_id))) => terminalization
                .commit_terminalization_with_snapshot(
                    run_id,
                    flow_id.clone(),
                    target.clone(),
                    run.status.clone(),
                    &run.flow_state,
                    &outcome.next_state,
                    vec![authority_input.clone()],
                )
                .await?
                .is_some(),
            (Some(next_status), None) => {
                run_store
                    .cas_run_snapshot_with_authority(
                        run_id,
                        run.status.clone(),
                        &run.flow_state,
                        next_status.clone(),
                        &outcome.next_state,
                        vec![authority_input.clone()],
                    )
                    .await?
            }
            (None, _) => {
                run_store
                    .cas_flow_state_with_authority(
                        run_id,
                        &run.flow_state,
                        &outcome.next_state,
                        vec![authority_input.clone()],
                    )
                    .await?
            }
        };
        if won {
            *authority = prepared;
            return Ok(true);
        }
        if attempt < 4 {
            tracing::debug!(attempt, context, "recovered flow CAS contention, retrying");
        }
    }
    Err(MobError::Internal(format!(
        "{context}: CAS contention after 5 attempts for recovered run {run_id}"
    )))
}

fn apply_recovered_flow_signal(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    signal: crate::machines::mob_machine::MobMachineSignal,
    context: &'static str,
) -> Result<(), MobError> {
    let signal_debug = format!("{signal:?}");
    let transition = authority.apply_signal(signal).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine recovered flow signal ({context}) rejected {signal_debug}: {error}"
        ))
    })?;
    let _ = transition;
    Ok(())
}

fn seed_mob_authority_restore_failures(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    restore_diagnostics: &HashMap<AgentIdentity, super::handle::RestoreFailureDiagnostic>,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;
    for (identity, diagnostic) in restore_diagnostics {
        apply_seeded_mob_signal(
            authority,
            mob_dsl::MobMachineSignal::RecoverMemberRestoreFailure {
                agent_identity: mob_dsl::AgentIdentity::from_domain(
                    &crate::ids::AgentIdentity::from(identity.as_str()),
                ),
                reason: diagnostic.reason.clone(),
            },
            "recover_member_restore_failure",
        )?;
    }
    Ok(())
}

struct RuntimeWiring {
    roster: Arc<RwLock<RosterAuthority>>,
    /// DSL authority pre-seeded from generated recovery replay on resume paths
    /// (and from scratch on create paths). Carried through the wiring so the
    /// actor receives membership-populated authority state before the first
    /// command is processed. Boxed so `RuntimeWiring` stays slim inside the
    /// async `resume()` future.
    dsl_authority: Box<crate::machines::mob_machine::MobMachineAuthority>,
    machine_state_watch_tx:
        tokio::sync::watch::Sender<crate::machines::mob_machine::MobMachineState>,
    restore_diagnostics:
        Arc<RwLock<HashMap<AgentIdentity, super::handle::RestoreFailureDiagnostic>>>,
    runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    supervisor_bridge: Arc<MobSupervisorBridge>,
    command_tx: mpsc::Sender<MobCommand>,
    command_rx: mpsc::Receiver<MobCommand>,
}

impl MobBuilder {
    /// Create a builder for a new mob.
    pub fn new(definition: MobDefinition, storage: MobStorage) -> Self {
        Self {
            mode: BuilderMode::Create(Arc::new(definition)),
            storage,
            session_service: None,
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: None,
            workgraph_service: None,
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
            default_external_tools_provider: None,
            spawn_member_customizer: None,
            owner_bridge_session_create_authority: None,
            realtime_session_factory: None,
        }
    }

    /// Create a builder from a mobpack definition + packed skill files.
    ///
    /// Any `SkillSource::Path` entries are resolved against `packed_skills` and
    /// converted to inline sources for runtime execution.
    pub fn from_mobpack(
        definition: MobDefinition,
        packed_skills: BTreeMap<String, Vec<u8>>,
        storage: MobStorage,
    ) -> Result<Self, MobError> {
        let definition = Self::lower_mobpack_definition(definition, &packed_skills)?;
        Ok(Self::new(definition, storage))
    }

    pub fn lower_mobpack_definition(
        mut definition: MobDefinition,
        packed_skills: &BTreeMap<String, Vec<u8>>,
    ) -> Result<MobDefinition, MobError> {
        for (skill_name, source) in &mut definition.skills {
            if let crate::definition::SkillSource::Path { path } = source {
                let bytes = packed_skills.get(path).ok_or_else(|| {
                    MobError::Internal(format!(
                        "mobpack skill path '{path}' for '{skill_name}' missing from archive"
                    ))
                })?;
                let content = String::from_utf8(bytes.clone()).map_err(|_| {
                    MobError::Internal(format!(
                        "mobpack skill path '{path}' for '{skill_name}' is not valid UTF-8"
                    ))
                })?;
                *source = crate::definition::SkillSource::Inline { content };
            }
        }
        Ok(definition)
    }

    /// Create a builder that resumes a mob from persisted events.
    pub fn for_resume(storage: MobStorage) -> Self {
        Self {
            mode: BuilderMode::Resume,
            storage,
            session_service: None,
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: None,
            workgraph_service: None,
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
            default_external_tools_provider: None,
            spawn_member_customizer: None,
            owner_bridge_session_create_authority: None,
            realtime_session_factory: None,
        }
    }

    /// Bind create-time owner bridge-session lifecycle through generated
    /// `MobMachine` authority.
    #[doc(hidden)]
    pub fn with_owner_bridge_session_create_authority(
        mut self,
        bridge_session_id: SessionId,
        destroy_on_owner_archive: bool,
        implicit_delegation_mob: bool,
    ) -> Self {
        self.owner_bridge_session_create_authority = Some(OwnerBridgeSessionCreateAuthority {
            bridge_session_id,
            destroy_on_owner_archive,
            implicit_delegation_mob,
        });
        self
    }

    /// Set the session service for creating meerkat sessions.
    ///
    /// The service must implement both `SessionService` and `MobSessionService`
    /// to provide comms runtime access for wiring operations. If no explicit
    /// runtime adapter override has been set yet, the builder seeds its
    /// canonical runtime adapter from `service.runtime_adapter()`.
    pub fn with_session_service(mut self, service: Arc<dyn MobSessionService>) -> Self {
        #[cfg(feature = "runtime-adapter")]
        if self.runtime_adapter.is_none() {
            self.runtime_adapter = service.runtime_adapter();
        }
        self.session_service = Some(service);
        self
    }

    /// Attach the canonical runtime adapter for the mob runtime.
    ///
    /// When set, this override is used consistently by both provisioning and
    /// autonomous-host comms-drain ingress instead of re-deriving an adapter
    /// from the session service at runtime.
    #[cfg(feature = "runtime-adapter")]
    pub fn with_runtime_adapter(mut self, adapter: Arc<meerkat_runtime::MeerkatMachine>) -> Self {
        self.runtime_adapter = Some(adapter);
        self
    }

    pub fn with_workgraph_service(mut self, service: Option<meerkat::WorkGraphService>) -> Self {
        self.workgraph_service = service;
        self
    }

    /// Allow non-persistent session services (for ephemeral/dev/test workflows).
    ///
    /// Default is `false`, which enforces the persistent-session contract.
    pub fn allow_ephemeral_sessions(mut self, allow: bool) -> Self {
        self.allow_ephemeral_sessions = allow;
        self
    }

    /// Control whether `resume()` sends an informational turn to the orchestrator.
    ///
    /// Default is `true`. CLI command rehydration can set this to `false` to avoid
    /// triggering extra orchestrator turns on every command invocation.
    pub fn notify_orchestrator_on_resume(mut self, notify: bool) -> Self {
        self.notify_orchestrator_on_resume = notify;
        self
    }

    /// Set a default LLM client override (primarily for testing).
    pub fn with_default_llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.default_llm_client = Some(client);
        self
    }

    /// Register a named Rust tool bundle for profile `tools.rust_bundles` wiring.
    pub fn register_tool_bundle(
        mut self,
        name: impl Into<String>,
        dispatcher: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        self.tool_bundles.insert(name.into(), dispatcher);
        self
    }

    /// Set a provider closure that is called at each member spawn to get a fresh
    /// snapshot of default external tools (e.g. callback tools from the SDK).
    pub fn with_default_external_tools_provider(
        mut self,
        provider: Option<crate::ExternalToolsProvider>,
    ) -> Self {
        self.default_external_tools_provider = provider;
        self
    }

    /// Register a pre-build customizer for every mob member spawn.
    pub fn with_spawn_member_customizer(
        mut self,
        customizer: Arc<dyn super::SpawnMemberCustomizer>,
    ) -> Self {
        self.spawn_member_customizer = Some(customizer);
        self
    }

    /// Inject a [`meerkat_client::RealtimeSessionFactory`] for mob-provisioned
    /// members that open a realtime channel (W2-E / issue #264).
    ///
    /// Tests use this to point mob members at a deterministic in-process
    /// mock (`mock_realtime_ws::RealtimeMockServer`) instead of a live
    /// OpenAI endpoint. The factory is carried through to [`MobHandle`]
    /// so test harnesses can wire it into a `RealtimeWsHost` bound to the
    /// same runtime.
    pub fn with_realtime_session_factory(
        mut self,
        factory: Arc<dyn meerkat_client::RealtimeSessionFactory>,
    ) -> Self {
        self.realtime_session_factory = Some(factory);
        self
    }

    /// Create the mob: emit MobCreated event, start the actor, return handle.
    #[cfg(feature = "runtime-adapter")]
    pub async fn create(self) -> Result<MobHandle, MobError> {
        let MobBuilder {
            mode,
            storage,
            session_service,
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter,
            workgraph_service,
            allow_ephemeral_sessions,
            notify_orchestrator_on_resume: _,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            spawn_member_customizer,
            owner_bridge_session_create_authority,
            realtime_session_factory,
        } = self;
        #[cfg(not(feature = "runtime-adapter"))]
        let runtime_adapter: RuntimeAdapterOption = None;

        let definition = match mode {
            BuilderMode::Create(definition) => definition,
            BuilderMode::Resume => {
                return Err(MobError::Internal(
                    "MobBuilder::create() cannot be used with for_resume(); call resume()"
                        .to_string(),
                ));
            }
        };
        let mut diagnostics = crate::validate::validate_definition(&definition);
        diagnostics.extend(crate::spec::SpecValidator::validate(&definition));
        let (errors, warnings) = crate::validate::partition_diagnostics(diagnostics);
        if !errors.is_empty() {
            return Err(MobError::DefinitionError(errors));
        }
        for warning in warnings {
            tracing::warn!(
                code = %warning.code,
                location = ?warning.location,
                "{}",
                warning.message
            );
        }
        let session_service = session_service
            .ok_or_else(|| MobError::Internal("session_service is required".into()))?;
        if !allow_ephemeral_sessions && !session_service.supports_persistent_sessions() {
            return Err(MobError::Internal(
                "session_service must satisfy persistent-session contract (REQ-MOB-030)"
                    .to_string(),
            ));
        }
        let runtime_adapter =
            canonical_runtime_adapter_for_session_service(&session_service, runtime_adapter)?;

        // §8: AutonomousHost profiles require a runtime adapter. Validate at
        // build time so Option<adapter> on the trait doesn't hide an ownership
        // requirement that only surfaces at spawn time.
        #[cfg(feature = "runtime-adapter")]
        {
            let has_autonomous = definition
                .profiles
                .values()
                .filter_map(|b| b.as_inline())
                .any(|p| p.runtime_mode == crate::MobRuntimeMode::AutonomousHost);
            if has_autonomous && runtime_adapter.is_none() {
                return Err(MobError::Internal(
                    "definition contains AutonomousHost profiles but no runtime adapter is available; \
                     provide one via with_runtime_adapter() or use a session service that implements \
                     runtime_adapter()"
                        .to_string(),
                ));
            }
        }

        let mut dsl_authority = Box::new(seed_mob_authority());
        let owner_bridge_event = match owner_bridge_session_create_authority.as_ref() {
            Some(binding) => bind_owner_bridge_session_authority_for_create(
                &mut dsl_authority,
                binding,
                "create_owner_bridge_session",
            )?,
            None => None,
        };
        seed_mob_definition_spawn_policy(
            &mut dsl_authority,
            &definition,
            "create_definition_spawn_policy",
        )?;
        finish_seeded_mob_authority_phase(&mut dsl_authority, MobState::Running)?;

        // Emit MobCreated event first
        let definition_for_event = (*definition).clone();
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition_for_event),
                },
            })
            .await?;
        if let Some(owner_bridge_event) = owner_bridge_event {
            storage
                .events
                .append(NewMobEvent {
                    mob_id: definition.id.clone(),
                    timestamp: None,
                    kind: owner_bridge_event,
                })
                .await?;
        }
        Self::sync_definition_with_spec_store(
            storage.specs.clone(),
            definition.id.clone(),
            definition.as_ref(),
        )
        .await?;
        Self::ensure_supervisor_authority(
            storage.runtime_metadata.clone(),
            definition.id.clone(),
            &mut dsl_authority,
        )
        .await?;
        let supervisor_authority = storage
            .runtime_metadata
            .load_supervisor_authority(&definition.id)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "missing supervisor runtime metadata for newly created mob '{}'",
                    definition.id
                ))
            })?;
        let supervisor_bridge = Arc::new(
            MobSupervisorBridge::new(
                &definition.id,
                supervisor_authority.clone(),
                definition
                    .backend
                    .external
                    .as_ref()
                    .and_then(|external| external.supervisor_bridge.clone()),
            )
            .await?,
        );

        Self::start_runtime(
            definition,
            Roster::new(),
            dsl_authority,
            storage.events.clone(),
            storage.runs.clone(),
            storage.runtime_metadata.clone(),
            supervisor_bridge,
            session_service,
            runtime_adapter,
            workgraph_service,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            spawn_member_customizer,
            storage.realm_profiles.clone(),
            realtime_session_factory,
        )
    }

    /// Resume a mob from persisted events.
    ///
    /// Resume behavior:
    /// - Recover definition from `MobCreated`.
    /// - Rebuild roster by replaying structural events.
    /// - Start actor/runtime in Running state.
    #[cfg(feature = "runtime-adapter")]
    pub async fn resume(self) -> Result<MobHandle, MobError> {
        let MobBuilder {
            mode,
            storage,
            session_service,
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter,
            workgraph_service,
            allow_ephemeral_sessions,
            notify_orchestrator_on_resume,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            spawn_member_customizer,
            owner_bridge_session_create_authority: _,
            realtime_session_factory,
        } = self;
        #[cfg(not(feature = "runtime-adapter"))]
        let runtime_adapter: RuntimeAdapterOption = None;

        if !matches!(mode, BuilderMode::Resume) {
            return Err(MobError::Internal(
                "MobBuilder::resume() requires MobBuilder::for_resume(storage)".to_string(),
            ));
        }

        let session_service = session_service
            .ok_or_else(|| MobError::Internal("session_service is required".into()))?;
        if !allow_ephemeral_sessions && !session_service.supports_persistent_sessions() {
            return Err(MobError::Internal(
                "session_service must satisfy persistent-session contract (REQ-MOB-030)"
                    .to_string(),
            ));
        }
        let runtime_adapter =
            canonical_runtime_adapter_for_session_service(&session_service, runtime_adapter)?;
        // §8 check deferred until after definition recovery — the definition
        // comes from the event log, so we can't check profiles before replay.
        let all_events = storage.events.replay_all().await?;

        // Use the last MobCreated event — reset appends a fresh MobCreated
        // to start a new epoch, so the latest one reflects the current definition.
        let definition = all_events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                MobEventKind::MobCreated { definition } => Some(*definition.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "cannot resume mob: no MobCreated event found in storage".to_string(),
                )
            })?;
        // §8: AutonomousHost profiles require a runtime adapter. Same check
        // as create(), but deferred until after definition recovery from events.
        let has_autonomous = definition
            .profiles
            .values()
            .filter_map(|b| b.as_inline())
            .any(|p| p.runtime_mode == crate::MobRuntimeMode::AutonomousHost);
        if has_autonomous && runtime_adapter.is_none() {
            return Err(MobError::Internal(
                "definition contains AutonomousHost profiles but no runtime adapter is available; \
                 provide one via with_runtime_adapter() or use a session service that implements \
                 runtime_adapter()"
                    .to_string(),
            ));
        }

        Self::sync_definition_with_spec_store(
            storage.specs.clone(),
            definition.id.clone(),
            &definition,
        )
        .await?;

        let definition = Arc::new(definition);
        let mut diagnostics = crate::validate::validate_definition(&definition);
        diagnostics.extend(crate::spec::SpecValidator::validate(definition.as_ref()));
        let (errors, warnings) = crate::validate::partition_diagnostics(diagnostics);
        if !errors.is_empty() {
            return Err(MobError::DefinitionError(errors));
        }
        for warning in warnings {
            tracing::warn!(
                code = %warning.code,
                location = ?warning.location,
                "{}",
                warning.message
            );
        }
        #[allow(unused_mut)]
        let mut mob_events: Vec<_> = all_events
            .into_iter()
            .filter(|event| event.mob_id == definition.id)
            .collect();
        // Select the current durable epoch (after the last MobReset, or all
        // events if no reset has occurred). Do this before
        // supervisor-authority recovery so destroy-finalizing storage can fail
        // closed instead of minting replacement live authority.
        let epoch_start = mob_events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |pos| pos + 1);
        let epoch_events = &mob_events[epoch_start..];
        let destroy_storage_finalizing = epoch_events
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobDestroyStorageFinalizing));
        let mut initial_dsl_authority = Box::new(seed_mob_authority());
        recover_owner_bridge_session_authority_from_history(
            &mut initial_dsl_authority,
            &mob_events,
        )?;
        seed_mob_authority_sync_from_events(&mut initial_dsl_authority, epoch_events, &definition)?;
        let resumed_state = seeded_mob_public_phase(initial_dsl_authority.state());
        // MobMachine owns supervisor authority. Runtime metadata is the
        // mechanical persistence projection. External-binding overlays are
        // compatibility projections only.
        if !destroy_storage_finalizing {
            Self::ensure_supervisor_authority(
                storage.runtime_metadata.clone(),
                definition.id.clone(),
                initial_dsl_authority.as_mut(),
            )
            .await?;
        }
        let supervisor_authority = match storage
            .runtime_metadata
            .load_supervisor_authority(&definition.id)
            .await?
        {
            Some(record) if record.protocol_version.is_supported() => {
                if destroy_storage_finalizing {
                    Self::recover_supervisor_authority(
                        initial_dsl_authority.as_mut(),
                        &record,
                        "destroy_storage_finalizing_resume",
                    )?;
                }
                record
            }
            Some(record) => {
                return Err(MobError::WiringError(format!(
                    "unsupported supervisor bridge protocol version {} (supported {:?}; default {})",
                    record.protocol_version,
                    super::bridge_protocol::supervisor_bridge_supported_protocol_versions(),
                    super::bridge_protocol::supervisor_bridge_default_protocol_version()
                )));
            }
            None if destroy_storage_finalizing => {
                return Err(MobError::Internal(format!(
                    "cannot resume mob '{}': destroy storage finalization is in progress but supervisor runtime metadata is absent",
                    definition.id
                )));
            }
            None => {
                return Err(MobError::Internal(format!(
                    "cannot resume mob '{}': missing supervisor runtime metadata",
                    definition.id
                )));
            }
        };
        let supervisor_bridge = Arc::new(
            MobSupervisorBridge::new(
                &definition.id,
                supervisor_authority,
                definition
                    .backend
                    .external
                    .as_ref()
                    .and_then(|external| external.supervisor_bridge.clone()),
            )
            .await?,
        );
        let mut roster = Roster::project(&mob_events);
        #[cfg(not(target_arch = "wasm32"))]
        Self::normalize_sessionless_backend_runtime_modes(&mut roster);
        let seeded_restore_diagnostics = HashMap::new();
        // Prepare shared runtime components early so resume reconciliation can
        // wire tool dispatchers for recreated sessions to the final actor channel.
        let roster_state = Arc::new(RwLock::new(RosterAuthority::new()));
        let (command_tx, command_rx) = mpsc::channel(MOB_COMMAND_CHANNEL_CAPACITY);
        let restore_diagnostics = Arc::new(RwLock::new(seeded_restore_diagnostics));
        let (machine_state_watch_tx, machine_state_watch_rx) =
            tokio::sync::watch::channel(initial_dsl_authority.state().clone());
        // Preview phase watch so the preview handle can answer status()
        // before the actor spawns. The real actor-side sender replaces
        // this once start_runtime_with_components owns the final pair.
        let (_preview_phase_tx, preview_phase_rx) = tokio::sync::watch::channel(resumed_state);
        let mut wiring = RuntimeWiring {
            roster: roster_state.clone(),
            dsl_authority: initial_dsl_authority,
            machine_state_watch_tx,
            restore_diagnostics: restore_diagnostics.clone(),
            runtime_metadata: storage.runtime_metadata.clone(),
            supervisor_bridge: supervisor_bridge.clone(),
            command_tx: command_tx.clone(),
            command_rx,
        };
        let preview_handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster_state.clone(),
            definition: definition.clone(),
            events: storage.events.clone(),
            run_store: storage.runs.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: session_service.clone(),
            runtime_adapter: runtime_adapter.clone(),
            restore_diagnostics,
            supervisor_bridge: supervisor_bridge.clone(),
            machine_state_watch_rx,
            phase_watch_rx: preview_phase_rx,
            realtime_session_factory: realtime_session_factory.clone(),
        };
        // session_service is still live here (not consumed until start_runtime_with_components)

        let seeded_topology_epoch = Arc::new(std::sync::atomic::AtomicU64::new(
            wiring.dsl_authority.state().topology_epoch,
        ));

        let mut per_spawn_external_tools_seed = BTreeMap::new();
        if resumed_state == MobState::Running {
            Self::reconcile_resume(
                &definition,
                &mut roster,
                &session_service,
                runtime_adapter.clone(),
                workgraph_service.clone(),
                supervisor_bridge.clone(),
                notify_orchestrator_on_resume,
                default_llm_client.clone(),
                &tool_bundles,
                wiring.dsl_authority.as_mut(),
                &seeded_topology_epoch,
                &preview_handle,
                &default_external_tools_provider,
                &spawn_member_customizer,
                storage.realm_profiles.clone(),
                storage.runtime_metadata.clone(),
                &mut per_spawn_external_tools_seed,
            )
            .await?;
        }

        seed_mob_authority_sync_from_flow_runs(
            &mut wiring.dsl_authority,
            storage.runs.clone(),
            storage.events.clone(),
            &definition.id,
        )
        .await?;
        let restore_diagnostics_snapshot = preview_handle.restore_diagnostics.read().await.clone();
        seed_mob_authority_restore_failures(
            &mut wiring.dsl_authority,
            &restore_diagnostics_snapshot,
        )?;
        let _ = wiring
            .machine_state_watch_tx
            .send(wiring.dsl_authority.state().clone());
        *wiring.roster.write().await = RosterAuthority::from_roster(roster);

        Ok(Self::start_runtime_with_components(
            definition,
            wiring,
            storage.events.clone(),
            storage.runs.clone(),
            session_service,
            runtime_adapter,
            workgraph_service,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            spawn_member_customizer,
            storage.realm_profiles.clone(),
            per_spawn_external_tools_seed,
            realtime_session_factory,
        ))
    }

    async fn sync_definition_with_spec_store(
        specs: Arc<dyn crate::store::MobSpecStore>,
        mob_id: MobId,
        definition: &MobDefinition,
    ) -> Result<(), MobError> {
        match specs.get_spec(&mob_id).await? {
            Some((stored, _revision)) if stored != *definition => Err(MobError::Internal(
                "persisted spec store definition does not match MobCreated runtime definition"
                    .to_string(),
            )),
            Some(_) => Ok(()),
            None => {
                let _ = specs.put_spec(&mob_id, definition, None).await?;
                Ok(())
            }
        }
    }

    async fn ensure_supervisor_authority(
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        mob_id: MobId,
        authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    ) -> Result<(), MobError> {
        let default_protocol_version =
            super::bridge_protocol::supervisor_bridge_default_protocol_version();
        match runtime_metadata.load_supervisor_authority(&mob_id).await? {
            None => {
                let record =
                    crate::store::SupervisorAuthorityRecord::generate(default_protocol_version);
                let mut prepared =
                    crate::machines::mob_machine::MobMachineAuthority::recover_from_state(
                        authority.state().clone(),
                    )
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "could not prepare supervisor authority provision: {error}"
                        ))
                    })?;
                let transition = crate::machines::mob_machine::MobMachineMutator::apply(
                    &mut prepared,
                    record.dsl_provision_input(),
                )
                .map_err(|error| {
                    MobError::Internal(format!(
                        "generated supervisor authority rejected provision: {error}"
                    ))
                })?;
                let persistence_authority =
                    crate::store::SupervisorAuthorityPersistenceAuthority::from_transition(
                        &record,
                        &transition,
                    )?;
                let inserted = runtime_metadata
                    .put_supervisor_authority_if_absent(&mob_id, &record, &persistence_authority)
                    .await?;
                if inserted {
                    *authority = prepared;
                } else {
                    let Some(existing) =
                        runtime_metadata.load_supervisor_authority(&mob_id).await?
                    else {
                        return Err(MobError::Internal(format!(
                            "supervisor authority initialization for mob '{mob_id}' lost the insert race but no record is present"
                        )));
                    };
                    if !existing.protocol_version.is_supported() {
                        return Err(MobError::WiringError(format!(
                            "unsupported supervisor bridge protocol version {} (supported {:?}; default {})",
                            existing.protocol_version,
                            super::bridge_protocol::supervisor_bridge_supported_protocol_versions(),
                            default_protocol_version
                        )));
                    }
                    Self::recover_supervisor_authority(
                        authority,
                        &existing,
                        "resume_after_insert_race",
                    )?;
                }
            }
            Some(record) if record.protocol_version.is_supported() => {
                Self::recover_supervisor_authority(authority, &record, "resume")?;
            }
            Some(record) => {
                return Err(MobError::WiringError(format!(
                    "unsupported supervisor bridge protocol version {} (supported {:?}; default {})",
                    record.protocol_version,
                    super::bridge_protocol::supervisor_bridge_supported_protocol_versions(),
                    default_protocol_version
                )));
            }
        }

        Ok(())
    }

    fn recover_supervisor_authority(
        authority: &mut crate::machines::mob_machine::MobMachineAuthority,
        record: &crate::store::SupervisorAuthorityRecord,
        context: &str,
    ) -> Result<(), MobError> {
        authority
            .apply_signal(record.dsl_recover_signal())
            .map(|_| ())
            .map_err(|error| {
                MobError::Internal(format!(
                    "generated supervisor authority rejected recovery ({context}): {error}"
                ))
            })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn normalize_sessionless_backend_runtime_modes(roster: &mut Roster) {
        let identities = roster
            .list_all()
            .map(|entry| entry.agent_identity.clone())
            .collect::<Vec<_>>();
        for identity in identities {
            if let Some(entry) = roster.get_by_identity_mut(&identity) {
                entry.runtime_mode = Self::normalize_runtime_mode_for_member_ref(
                    entry.runtime_mode,
                    Some(&entry.member_ref),
                );
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn normalize_runtime_mode_for_member_ref(
        runtime_mode: crate::MobRuntimeMode,
        member_ref: Option<&crate::event::MemberRef>,
    ) -> crate::MobRuntimeMode {
        match member_ref {
            Some(crate::event::MemberRef::BackendPeer {
                session_id: None, ..
            }) => crate::MobRuntimeMode::TurnDriven,
            _ => runtime_mode,
        }
    }

    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::too_many_arguments)]
    async fn reconcile_resume(
        definition: &Arc<MobDefinition>,
        roster: &mut Roster,
        session_service: &Arc<dyn MobSessionService>,
        runtime_adapter: RuntimeAdapterOption,
        workgraph_service: Option<meerkat::WorkGraphService>,
        supervisor_bridge: Arc<MobSupervisorBridge>,
        notify_orchestrator_on_resume: bool,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        dsl_authority: &mut crate::machines::mob_machine::MobMachineAuthority,
        topology_epoch: &Arc<std::sync::atomic::AtomicU64>,
        tool_handle: &MobHandle,
        default_external_tools_provider: &Option<crate::ExternalToolsProvider>,
        spawn_member_customizer: &Option<Arc<dyn super::SpawnMemberCustomizer>>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        per_spawn_external_tools_seed: &mut BTreeMap<AgentIdentity, Arc<dyn AgentToolDispatcher>>,
    ) -> Result<(), MobError> {
        // Every exact operation target is potentially already complete even if
        // its observation checkpoint timed out. Resume must defer old-authority
        // authorization/trust reconciliation for the full target set, not only
        // peers with checkpointed accepted receipts; the rotation retry owns
        // observation and final activation.
        let pending_supervisor_operation_peer_ids: std::collections::BTreeSet<String> =
            supervisor_bridge
                .authority()
                .await
                .pending_rotation
                .as_ref()
                .map(|pending| {
                    pending
                        .accepted_peer_ids
                        .iter()
                        .cloned()
                        .chain(pending.member_targets.keys().cloned())
                        .collect()
                })
                .unwrap_or_default();
        let provisioner = MultiBackendProvisioner::new(
            session_service.clone(),
            runtime_adapter.clone(),
            workgraph_service,
            definition.backend.external.clone(),
            supervisor_bridge,
        );
        let listed_sessions = session_service
            .list(meerkat_core::service::SessionQuery::default())
            .await?;
        // Live bridge existence is session-service-owned truth. Do not infer it
        // from SessionSummary presence or `is_active`, because persisted-only
        // summaries are not live and idle live sessions are not "active".
        let mut active_ids = std::collections::HashSet::new();
        for summary in &listed_sessions {
            if session_service
                .has_live_session(&summary.session_id)
                .await?
            {
                active_ids.insert(summary.session_id.clone());
            }
        }

        let roster_entries = roster.list().cloned().collect::<Vec<_>>();
        let machine_session_ids = dsl_authority
            .state()
            .member_session_bindings
            .values()
            // A releasing retirement-start deliberately removes the ordinary
            // member binding, but its exact session remains machine-owned by
            // the durable pending-retirement obligation. Treat that session as
            // claimed during resume so orphan reconciliation cannot archive it
            // before the routed retirement retry runs.
            .chain(
                dsl_authority
                    .state()
                    .runtime_retire_pending_sessions
                    .values(),
            )
            .filter_map(|session_id| meerkat_core::types::SessionId::parse(&session_id.0).ok())
            .collect::<std::collections::HashSet<_>>();

        // Archive orphan sessions that are active but not present in
        // MobMachine's recovered identity→session binding authority.
        for session_id in active_ids.difference(&machine_session_ids) {
            if session_service
                .session_belongs_to_mob(session_id, &definition.id)
                .await
                && let Err(error) = session_service
                    .archive_with_mob_lifecycle_authority(session_id)
                    .await
                && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
            {
                return Err(error.into());
            }
        }
        // Recreate missing sessions referenced by MemberSpawned events.
        for entry in &roster_entries {
            let dsl_identity =
                crate::machines::mob_machine::AgentIdentity::from_domain(&entry.agent_identity);
            let Some(mut bridge_session_id) = dsl_authority
                .state()
                .member_session_bindings
                .get(&dsl_identity)
                .and_then(|session_id| meerkat_core::types::SessionId::parse(&session_id.0).ok())
            else {
                continue;
            };
            if active_ids.contains(&bridge_session_id) {
                continue;
            }
            let record_restore_failure = |bridge_session_id: meerkat_core::types::SessionId,
                                          reason: String| async {
                tool_handle.restore_diagnostics.write().await.insert(
                    entry.agent_identity.clone(),
                    super::handle::RestoreFailureDiagnostic {
                        bridge_session_id: Some(bridge_session_id),
                        reason,
                    },
                );
            };

            let mut restore_spec =
                super::SpawnMemberSpec::new(entry.role.clone(), entry.agent_identity.clone());
            restore_spec.runtime_mode = Some(entry.runtime_mode);
            restore_spec.labels = Some(entry.labels.clone());
            restore_spec.override_profile = entry.effective_profile_override.clone();
            if let Some(customizer) = spawn_member_customizer.as_ref() {
                let ctx = super::SpawnCustomizationContext {
                    mob_id: definition.id.clone(),
                    spawn_source: super::SpawnSource::Resume,
                    spawner_identity: None,
                    spawner_runtime_id: None,
                    requested_profile: restore_spec.role_name.clone(),
                };
                customizer.customize_spawn(&ctx, &mut restore_spec)?;
            }
            if restore_spec.identity != entry.agent_identity {
                return Err(MobError::Internal(format!(
                    "spawn customizer cannot change resume restore identity from '{}' to '{}'",
                    entry.agent_identity, restore_spec.identity
                )));
            }
            if restore_spec.role_name != entry.role {
                return Err(MobError::Internal(format!(
                    "spawn customizer cannot change resume restore profile for '{}' from '{}' to '{}'",
                    entry.agent_identity, entry.role, restore_spec.role_name
                )));
            }
            // Seed the actor's retention map so a later machine-authorized
            // revival recomposes the customizer-supplied per-spawn overlay.
            if let Some(tools) = restore_spec.external_tools.clone() {
                per_spawn_external_tools_seed.insert(entry.agent_identity.clone(), tools);
            }
            let restore_profile_override = restore_spec.override_profile.clone();
            let restore_labels = restore_spec
                .labels
                .clone()
                .unwrap_or_else(|| entry.labels.clone());
            if let Some(roster_entry) = roster.get_by_identity_mut(&entry.agent_identity) {
                roster_entry.effective_profile_override = restore_profile_override.clone();
            }

            if matches!(entry.member_ref, MemberRef::Session { .. })
                && session_service.supports_persistent_sessions()
            {
                let stored_session = match session_service
                    .load_persisted_session(&bridge_session_id)
                    .await?
                {
                    Some(stored_session) => stored_session,
                    None => {
                        if let Some((replacement_session_id, replacement_session)) =
                            latest_persisted_session_for_member(
                                session_service.as_ref(),
                                &listed_sessions,
                                &bridge_session_id,
                                &definition.id,
                                &entry.role,
                                &entry.agent_identity,
                            )
                            .await?
                        {
                            apply_seeded_member_session_binding(
                                dsl_authority,
                                &entry.agent_identity,
                                &entry.agent_runtime_id,
                                &replacement_session_id,
                                "resume_repoint_missing_member_session_binding",
                            )?;
                            tool_handle
                                .events
                                .append(crate::event::NewMobEvent {
                                    mob_id: definition.id.clone(),
                                    timestamp: None,
                                    kind: MobEventKind::MemberSessionBindingRecovered(
                                        crate::event::MemberSessionBindingRecoveredEvent::new(
                                            entry.agent_identity.clone(),
                                            entry.agent_runtime_id.clone(),
                                            replacement_session_id.clone(),
                                        ),
                                    ),
                                })
                                .await?;
                            bridge_session_id = replacement_session_id;
                            let _ = roster.set_bridge_session_id(
                                &entry.agent_identity,
                                bridge_session_id.clone(),
                            );
                            if active_ids.contains(&bridge_session_id) {
                                continue;
                            }
                            replacement_session
                        } else {
                            record_restore_failure(
                                bridge_session_id.clone(),
                                format!(
                                    "missing durable session snapshot for '{bridge_session_id}'"
                                ),
                            )
                            .await;
                            continue;
                        }
                    }
                };
                // Prefer customizer/roster effective_profile_override on restore for lifecycle safety.
                let mut profile = if let Some(ref p) = restore_profile_override {
                    p.clone()
                } else {
                    definition
                        .resolve_profile(&entry.role, realm_profile_store.as_ref())
                        .await?
                };
                if restore_spec.inherited_tool_filter.is_some()
                    && restore_profile_override.is_none()
                {
                    build::open_profile_tool_categories_for_inherited_filter(&mut profile);
                }
                authorize_spawn_profile_material(
                    dsl_authority,
                    &entry.agent_identity,
                    &entry.role,
                    &profile,
                    "resume_existing_member_profile_authority",
                )?;
                let profile = &profile;
                let default_ext = default_external_tools_provider.as_ref().and_then(|p| p());
                let resumed_config =
                    build::build_resumed_agent_config(build::BuildResumedAgentConfigParams {
                        base: build::BuildAgentConfigParams {
                            mob_id: &definition.id,
                            profile_name: &entry.role,
                            agent_identity: &entry.agent_identity,
                            profile,
                            definition,
                            external_tools: compose_external_tools_for_profile(
                                profile,
                                tool_bundles,
                                tool_handle.clone(),
                                default_ext,
                                restore_spec.external_tools.clone(),
                                None,
                            )?,
                            context: restore_spec.context.clone(),
                            labels: Some(restore_labels.clone()),
                            additional_instructions: restore_spec.additional_instructions.clone(),
                            shell_env: restore_spec.shell_env.clone(),
                            mob_tool_authority_context: None,
                            inherited_tool_filter: restore_spec.inherited_tool_filter.clone(),
                            tool_access_policy: restore_spec.tool_access_policy.clone(),
                            system_prompt_override: restore_spec.system_prompt_override.clone(),
                        },
                        expected_session_id: &bridge_session_id,
                        resumed_session: stored_session,
                    })
                    .await;
                let mut resumed_config = match resumed_config {
                    Ok(config) => config,
                    Err(error) => {
                        record_restore_failure(bridge_session_id.clone(), error.to_string()).await;
                        continue;
                    }
                };
                resumed_config.keep_alive =
                    entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
                if let Some(ref auth_binding) = restore_spec.auth_binding {
                    resumed_config.auth_binding = Some(auth_binding.clone());
                }
                let reconcile_client: Arc<dyn LlmClient> = default_llm_client
                    .clone()
                    .unwrap_or_else(|| Arc::new(meerkat_client::TestClient::default()));
                resumed_config.llm_client_override = Some(reconcile_client);
                let prompt = format!(
                    "You have been spawned as '{}' (role: {}) in mob '{}'.",
                    entry.agent_identity, entry.role, definition.id
                );
                let req = build::to_create_session_request(&resumed_config, prompt.into());
                let peer_name = match super::actor::render_member_comms_name(
                    definition.id.as_str(),
                    entry.role.as_str(),
                    entry.agent_identity.as_str(),
                ) {
                    Ok(name) => name,
                    Err(error) => {
                        record_restore_failure(
                            bridge_session_id.clone(),
                            format!(
                                "failed to render comms name for resumed member '{}': {error}",
                                entry.agent_identity
                            ),
                        )
                        .await;
                        continue;
                    }
                };
                let mut provision_authority =
                    match crate::machines::mob_machine::MobMachineAuthority::recover_from_state(
                        dsl_authority.state().clone(),
                    ) {
                        Ok(authority) => authority,
                        Err(error) => {
                            record_restore_failure(bridge_session_id.clone(), format!(
                                "failed to recover MobMachine authority for session provision owner '{}': {error}",
                                entry.agent_identity
                            ))
                            .await;
                            continue;
                        }
                    };
                let generated_self_owned_operation_owner =
                    match authorize_seeded_session_provision_operation_owner(
                        &mut provision_authority,
                        &entry.agent_identity,
                        &entry.agent_runtime_id,
                        &bridge_session_id,
                        "resume_existing_member_session_provision_owner",
                    ) {
                        Ok(owner) => owner,
                        Err(error) => {
                            record_restore_failure(bridge_session_id.clone(), format!(
                                "failed to authorize recovered session provision owner '{}': {error}",
                                entry.agent_identity
                            ))
                            .await;
                            continue;
                        }
                    };
                match provisioner
                    .provision_member(super::provisioner::ProvisionMemberRequest {
                        create_session: req,
                        binding: crate::RuntimeBinding::Session,
                        peer_name,
                        owner_bridge_session_id: None,
                        ops_registry: None,
                        generated_self_owned_operation_owner: Some(
                            generated_self_owned_operation_owner,
                        ),
                    })
                    .await
                {
                    Ok(receipt) => {
                        let created_bridge_session_id = receipt
                            .member_ref
                            .bridge_session_id()
                            .cloned()
                            .ok_or_else(|| {
                                MobError::Internal(format!(
                                    "resume reconciliation provisioned non-session member for '{}'",
                                    entry.agent_identity
                                ))
                            })?;
                        *dsl_authority = provision_authority;
                        if let Err(error) = apply_seeded_member_addressability(
                            dsl_authority,
                            &entry.agent_identity,
                            &entry.agent_runtime_id,
                            entry.fence_token,
                            &entry.role,
                            entry.runtime_mode,
                            profile.external_addressable,
                            "resume_recovered_member_addressability",
                        ) {
                            record_restore_failure(bridge_session_id.clone(), format!(
                                "MobMachine rejected recovered member addressability for '{}': {error}",
                                entry.agent_identity
                            ))
                            .await;
                            continue;
                        }
                        if let Err(error) = apply_seeded_member_session_binding(
                            dsl_authority,
                            &entry.agent_identity,
                            &entry.agent_runtime_id,
                            &created_bridge_session_id,
                            "resume_recovered_member_session_binding",
                        ) {
                            record_restore_failure(bridge_session_id.clone(), format!(
                                "MobMachine rejected recovered bridge session '{created_bridge_session_id}': {error}"
                            ))
                            .await;
                            continue;
                        }
                        let _ = roster.set_bridge_session_id(
                            &entry.agent_identity,
                            created_bridge_session_id.clone(),
                        );
                        tool_handle
                            .restore_diagnostics
                            .write()
                            .await
                            .remove(&entry.agent_identity);
                    }
                    Err(error) => {
                        record_restore_failure(
                            bridge_session_id.clone(),
                            format!(
                                "failed to restore durable session '{bridge_session_id}': {error}"
                            ),
                        )
                        .await;
                    }
                }
                continue;
                // Ephemeral services can still fall back to fresh-create.
            }
            let mut profile = if let Some(ref p) = restore_profile_override {
                p.clone()
            } else {
                definition
                    .resolve_profile(&entry.role, realm_profile_store.as_ref())
                    .await?
            };
            if restore_spec.inherited_tool_filter.is_some() && restore_profile_override.is_none() {
                build::open_profile_tool_categories_for_inherited_filter(&mut profile);
            }
            authorize_spawn_profile_material(
                dsl_authority,
                &entry.agent_identity,
                &entry.role,
                &profile,
                "resume_recreate_member_profile_authority",
            )?;
            let default_ext_fresh = default_external_tools_provider.as_ref().and_then(|p| p());
            let mut config = build::build_agent_config(build::BuildAgentConfigParams {
                mob_id: &definition.id,
                profile_name: &entry.role,
                agent_identity: &entry.agent_identity,
                profile: &profile,
                definition,
                external_tools: compose_external_tools_for_profile(
                    &profile,
                    tool_bundles,
                    tool_handle.clone(),
                    default_ext_fresh,
                    restore_spec.external_tools.clone(),
                    None,
                )?,
                context: restore_spec.context.clone(),
                labels: Some(restore_labels.clone()),
                additional_instructions: restore_spec.additional_instructions.clone(),
                shell_env: restore_spec.shell_env.clone(),
                mob_tool_authority_context: None,
                inherited_tool_filter: restore_spec.inherited_tool_filter.clone(),
                tool_access_policy: restore_spec.tool_access_policy.clone(),
                system_prompt_override: restore_spec.system_prompt_override.clone(),
            })
            .await?;
            config.keep_alive = entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
            if let Some(ref auth_binding) = restore_spec.auth_binding {
                config.auth_binding = Some(auth_binding.clone());
            }
            // Resume reconciliation needs live comms runtimes, but this path is
            // infrastructure restoration and should not consume provider quota.
            // If no explicit override is configured, use the local test client
            // for deterministic, no-network bootstrap turns.
            let reconcile_client: Arc<dyn LlmClient> = default_llm_client
                .clone()
                .unwrap_or_else(|| Arc::new(meerkat_client::TestClient::default()));
            config.llm_client_override = Some(reconcile_client);
            let prompt = format!(
                "You have been spawned as '{}' (role: {}) in mob '{}'.",
                entry.agent_identity, entry.role, definition.id
            );
            let req = build::to_create_session_request(&config, prompt.into());
            let peer_name = super::actor::render_member_comms_name(
                definition.id.as_str(),
                entry.role.as_str(),
                entry.agent_identity.as_str(),
            )?;
            let mut provision_request = super::provisioner::ProvisionMemberRequest {
                create_session: req,
                binding: crate::RuntimeBinding::Session,
                peer_name,
                owner_bridge_session_id: None,
                ops_registry: None,
                generated_self_owned_operation_owner: None,
            };
            let admitted_bridge_session_id =
                super::actor::admit_bridge_session_for_spawn(&mut provision_request.create_session);
            let mut provision_authority =
                crate::machines::mob_machine::MobMachineAuthority::recover_from_state(
                    dsl_authority.state().clone(),
                )
                .map_err(|error| {
                    MobError::Internal(format!(
                        "failed to recover MobMachine authority for fresh session provision owner '{}': {error}",
                        entry.agent_identity
                    ))
                })?;
            provision_request.generated_self_owned_operation_owner =
                Some(authorize_seeded_session_provision_operation_owner(
                    &mut provision_authority,
                    &entry.agent_identity,
                    &entry.agent_runtime_id,
                    &admitted_bridge_session_id,
                    "resume_fresh_member_session_provision_owner",
                )?);
            let receipt = provisioner.provision_member(provision_request).await?;
            let created_bridge_session_id = receipt
                .member_ref
                .bridge_session_id()
                .cloned()
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "resume reconciliation provisioned non-session member for '{}'",
                        entry.agent_identity
                    ))
                })?;
            *dsl_authority = provision_authority;
            apply_seeded_member_addressability(
                dsl_authority,
                &entry.agent_identity,
                &entry.agent_runtime_id,
                entry.fence_token,
                &entry.role,
                entry.runtime_mode,
                profile.external_addressable,
                "resume_fresh_member_addressability",
            )?;
            apply_seeded_member_session_binding(
                dsl_authority,
                &entry.agent_identity,
                &entry.agent_runtime_id,
                &created_bridge_session_id,
                "resume_fresh_member_session_binding",
            )?;
            let _ = roster
                .set_bridge_session_id(&entry.agent_identity, created_bridge_session_id.clone());
            tool_handle
                .restore_diagnostics
                .write()
                .await
                .remove(&entry.agent_identity);
        }

        // Re-establish trust only from MobMachine-owned wiring. Roster replay
        // supplies member metadata, but it is not a behavior authority for
        // adding or pruning live comms trust.
        let mut entries = roster.list().cloned().collect::<Vec<_>>();
        let machine_wiring_edges = dsl_authority.state().wiring_edges.clone();
        let machine_external_peer_edges = dsl_authority.state().external_peer_edges.clone();
        let restore_diagnostics_snapshot = tool_handle.restore_diagnostics.read().await.clone();
        seed_mob_authority_restore_failures(dsl_authority, &restore_diagnostics_snapshot)?;
        let mob_owner_token = dsl_authority.generated_authority_owner_token();
        let broken_members = restore_diagnostics_snapshot
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        #[cfg(feature = "runtime-adapter")]
        {
            for entry in &entries {
                if broken_members.contains(&entry.agent_identity) {
                    continue;
                }
                let Some(bridge_session_id) = entry.member_ref.bridge_session_id() else {
                    continue;
                };
                if !active_ids.contains(bridge_session_id) {
                    continue;
                }
                let Some(adapter) = runtime_adapter.as_ref() else {
                    return Err(MobError::Internal(format!(
                        "resume operation owner binding for active member '{}' requires MeerkatMachine runtime authority",
                        entry.agent_identity
                    )));
                };
                bind_seeded_session_operation_owner_context(
                    &provisioner,
                    adapter,
                    dsl_authority,
                    entry,
                    bridge_session_id,
                    "resume_active_member_operation_owner_binding",
                )
                .await?;
            }
        }
        for entry in &entries {
            let dsl_identity =
                crate::machines::mob_machine::AgentIdentity::from_domain(&entry.agent_identity);
            if recovered_endpoint_runtime_is_retiring(dsl_authority.state(), &dsl_identity) {
                // Retirement replay restored the exact endpoint from the
                // durable start marker. Never replace it with a freshly
                // materialized runtime identity during resume: reciprocal
                // cleanup must target the trust row for the retiring runtime.
                continue;
            }
            if broken_members.contains(&entry.agent_identity) {
                let _ = roster.set_comms_identity(&entry.agent_identity, None, None);
                continue;
            }
            if let Some(comms_a) = provisioner.comms_runtime(&entry.member_ref).await {
                let peer_id_a = comms_a.peer_id().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume requires peer id for wired member '{}'",
                        entry.agent_identity
                    ))
                })?;
                let key_a = comms_a.public_key().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume requires public key for wired member '{}'",
                        entry.agent_identity
                    ))
                })?;
                let _ = roster.set_comms_identity(
                    &entry.agent_identity,
                    Some(peer_id_a),
                    Some(key_a.clone()),
                );
                let name_a = super::actor::render_member_comms_name(
                    definition.id.as_str(),
                    entry.role.as_str(),
                    entry.agent_identity.as_str(),
                )?;
                let spec = provisioner
                    .trusted_peer_spec(&entry.member_ref, &name_a, &key_a)
                    .await?;
                register_seeded_member_peer(
                    dsl_authority,
                    &entry.agent_identity,
                    &spec,
                    "resume_register_member_peer",
                )?;
            } else if let MemberRef::BackendPeer {
                peer_id,
                session_id: None,
                ..
            } = &entry.member_ref
            {
                let name = super::actor::render_member_comms_name(
                    definition.id.as_str(),
                    entry.role.as_str(),
                    entry.agent_identity.as_str(),
                )?;
                let spec = provisioner
                    .trusted_peer_spec(&entry.member_ref, &name, peer_id)
                    .await?;
                register_seeded_member_peer(
                    dsl_authority,
                    &entry.agent_identity,
                    &spec,
                    "resume_register_backend_member_peer",
                )?;
            }
        }
        for entry in &entries {
            let dsl_identity =
                crate::machines::mob_machine::AgentIdentity::from_domain(&entry.agent_identity);
            if recovered_endpoint_runtime_is_retiring(dsl_authority.state(), &dsl_identity) {
                // Retirement replay owns the remaining remote effects. Never
                // re-authorize or rebind its supervisor during resume: doing
                // so would undo a completed revoke or consume a redacted
                // bootstrap secret before the exact cleanup retry runs.
                continue;
            }
            if broken_members.contains(&entry.agent_identity)
                || provisioner.comms_runtime(&entry.member_ref).await.is_some()
            {
                continue;
            }
            if !matches!(
                entry.member_ref,
                MemberRef::BackendPeer {
                    session_id: None,
                    ..
                }
            ) {
                continue;
            }
            if peer_only_member_has_pending_supervisor_operation(
                &pending_supervisor_operation_peer_ids,
                &entry.member_ref,
            ) {
                continue;
            }
            let report = provisioner
                .reconcile_peer_only_trust(&entry.member_ref, None, None)
                .await?;
            if let Some(rebind_observation) = report.rebind_required {
                // The provisioner surfaced the raw rejection cause without
                // classifying it. MobMachine — via the seeded `dsl_authority` we
                // own here — decides whether the cause is recoverable by rebind.
                let should_rebind = classify_seeded_bridge_rejection_recovery(
                    dsl_authority,
                    rebind_observation.rejection_cause,
                    "resume_peer_only_rebind_classify_recovery",
                )?;
                if !should_rebind {
                    return Err(MobError::BridgeCommandRejected {
                        cause: rebind_observation.rejection_cause,
                        reason: format!(
                            "resume peer-only supervisor authorization for '{}' was rejected with a fatal cause",
                            entry.agent_identity
                        ),
                    });
                }
                let (updated_member_ref, rebind_authority) =
                    apply_resume_peer_only_rebind_authority(
                        dsl_authority,
                        roster,
                        &runtime_metadata,
                        &definition.id,
                        &entry.agent_identity,
                        &rebind_observation,
                        "resume_peer_only_rebind_authorize_member_peer",
                    )
                    .await?;
                let second_report = provisioner
                    .reconcile_peer_only_trust(&updated_member_ref, None, Some(&rebind_authority))
                    .await?;
                if second_report.rebind_required.is_some() {
                    return Err(MobError::WiringError(format!(
                        "resume peer-only rebind for '{}' was rejected after MobMachine authority",
                        entry.agent_identity
                    )));
                }
            }
        }
        entries = roster.list().cloned().collect::<Vec<_>>();
        let mut resume_trust_mutations = Vec::new();
        for entry in &entries {
            if broken_members.contains(&entry.agent_identity) {
                continue;
            }
            let local_comms = provisioner.comms_runtime(&entry.member_ref).await;
            let current_peers = match local_comms.as_ref() {
                Some(comms_a) => comms_a.peers().await,
                None => Vec::new(),
            };
            let local_peer_id = local_comms.as_ref().and_then(|comms| comms.peer_id());
            let mut desired_trust = Vec::new();

            let local_dsl_identity =
                crate::machines::mob_machine::AgentIdentity::from_domain(&entry.agent_identity);
            for edge in &machine_wiring_edges {
                let peer_dsl_identity = if edge.a == local_dsl_identity {
                    &edge.b
                } else if edge.b == local_dsl_identity {
                    &edge.a
                } else {
                    continue;
                };
                // A durable retirement-start marker means scoped topology
                // cleanup may already have removed one side before the actor
                // crashed. Recreating either side here would undo that
                // machine-authorized cleanup. Existing trust is deliberately
                // left untouched so the retirement retry can finish removing
                // it through the same scoped path.
                if !recovered_member_edge_allows_trust_repair(dsl_authority.state(), edge) {
                    continue;
                }
                let peer_identity = AgentIdentity::from(peer_dsl_identity.0.as_str());
                let peer_member_identity = AgentIdentity::from(peer_identity.as_str());
                let peer_entry = roster.get(&peer_member_identity).cloned().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume machine wiring target '{}' missing for '{}'",
                        peer_identity, entry.agent_identity
                    ))
                })?;
                let name_b = super::actor::render_member_comms_name(
                    definition.id.as_str(),
                    peer_entry.role.as_str(),
                    peer_entry.agent_identity.as_str(),
                )?;
                if broken_members.contains(&peer_member_identity) {
                    if let (Some(comms_a), Some(local_peer_id)) =
                        (local_comms.as_ref(), local_peer_id.as_ref())
                    {
                        let generated_peers = comms_a
                            .trusted_peer_projection_snapshot_for_source(
                                meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
                            )
                            .await
                            .map_err(|error| {
                                MobError::WiringError(format!(
                                    "resume failed to read generated trust for broken member '{}': {error}",
                                    peer_entry.agent_identity
                                ))
                            })?;
                        if let Some(stale_peer) = generated_peers
                            .iter()
                            .find(|peer| peer.name.as_str() == name_b)
                            .cloned()
                        {
                            let observed_endpoint =
                                crate::machines::mob_machine::MemberPeerEndpoint::from(&stale_peer);
                            match dsl_authority
                                .state()
                                .member_peer_endpoints
                                .get(peer_dsl_identity)
                            {
                                Some(existing) if existing != &observed_endpoint => {
                                    return Err(MobError::WiringError(format!(
                                        "resume generated trust disagrees on the retained peer endpoint for broken member '{}'",
                                        peer_entry.agent_identity
                                    )));
                                }
                                Some(_) => {}
                                None => register_seeded_member_peer(
                                    dsl_authority,
                                    &peer_entry.agent_identity,
                                    &stale_peer,
                                    "resume_register_broken_member_peer_from_generated_trust",
                                )?,
                            }
                            match resume_member_observed_cleanup_authority(
                                dsl_authority,
                                topology_epoch,
                                edge,
                                &entry.agent_identity,
                                local_peer_id,
                                &peer_entry.agent_identity,
                                &stale_peer.peer_id,
                                "resume_member_trust_cleanup_observed",
                            ) {
                                Ok(authority) => {
                                    resume_trust_mutations.push(ResumeTrustMutation {
                                        comms: Arc::clone(comms_a),
                                        operation: ResumeTrustMutationOperation::Remove(
                                            stale_peer.peer_id.to_string(),
                                        ),
                                        authority,
                                    });
                                }
                                Err(error) => return Err(error),
                            }
                        }
                    }
                    continue;
                }
                let peer_endpoint = dsl_authority
                    .state()
                    .member_peer_endpoints
                    .get(peer_dsl_identity)
                    .ok_or_else(|| {
                        MobError::WiringError(format!(
                            "resume machine wiring target '{}' lacks MobMachine peer endpoint for '{}'",
                            peer_identity, entry.agent_identity
                        ))
                    })?;
                let spec = trusted_peer_descriptor_from_dsl_member_endpoint(peer_endpoint)?;
                desired_trust.push(ResumeDesiredTrust {
                    spec,
                    source: ResumeTrustSource::Member(edge.clone()),
                });
            }

            for edge in &machine_external_peer_edges {
                if edge.local == local_dsl_identity
                    && !recovered_endpoint_runtime_is_retiring(dsl_authority.state(), &edge.local)
                {
                    let spec = trusted_peer_descriptor_from_dsl_external_endpoint(&edge.endpoint)?;
                    desired_trust.push(ResumeDesiredTrust {
                        spec,
                        source: ResumeTrustSource::External {
                            key: crate::machines::mob_machine::ExternalPeerKey::new(
                                edge.local.clone(),
                                edge.endpoint.name.clone(),
                            ),
                            edge: edge.clone(),
                        },
                    });
                }
            }

            // D31 resume wire-restoration: install trust for every edge still
            // present in MobMachine authority. Stale trust is not pruned here:
            // removing live trust requires a generated revoke/unwire
            // authority path, not a resume-time projection diff.
            let Some(comms_a) = local_comms else {
                if desired_trust.is_empty() {
                    continue;
                }
                if peer_only_member_has_pending_supervisor_operation(
                    &pending_supervisor_operation_peer_ids,
                    &entry.member_ref,
                ) {
                    continue;
                }
                // Peer-only external members have no local comms runtime on
                // the supervisor side; their trust lives on the remote
                // process, so resume reconciles it through the supervisor
                // bridge instead of mutating local state.
                // The V3 handoff carries the complete overlay and replaces
                // the remote projection atomically. If any connected edge is
                // retiring, even a filtered add would either recreate that
                // edge or remove surviving trust that scoped cleanup still
                // needs. Defer the whole peer-only overlay reconcile instead.
                if !recovered_peer_only_overlay_allows_trust_reconcile(
                    dsl_authority.state(),
                    &machine_wiring_edges,
                    &local_dsl_identity,
                ) {
                    continue;
                }
                let desired_peer_trust = peer_only_trust_overlay_from_mob_machine(
                    dsl_authority,
                    topology_epoch,
                    &entry.agent_identity,
                    "resume_peer_only_trust_overlay",
                )?;
                let report = provisioner
                    .reconcile_peer_only_trust(&entry.member_ref, Some(&desired_peer_trust), None)
                    .await?;
                if let Some(rebind_observation) = report.rebind_required {
                    // The rebind prepass already reconciled supervisor authority
                    // for this member, so a rejection here is bubbled up with
                    // its raw cause; the recoverable-vs-fatal verdict is
                    // MobMachine-owned and not re-derived in this trust-overlay
                    // reconcile path.
                    return Err(MobError::BridgeCommandRejected {
                        cause: rebind_observation.rejection_cause,
                        reason: format!(
                            "resume peer-only trust reconcile for '{}' was rejected after MobMachine rebind prepass",
                            entry.agent_identity
                        ),
                    });
                }
                continue;
            };
            for desired in &desired_trust {
                let spec_peer_id = desired.spec.peer_id.to_string();
                if current_peers
                    .iter()
                    .any(|entry| entry.peer_id.to_string() == spec_peer_id)
                {
                    continue;
                }
                let repair_authority = match &desired.source {
                    ResumeTrustSource::Member(edge) => {
                        let transition = apply_seeded_mob_input_collect_transition(
                            dsl_authority,
                            crate::machines::mob_machine::MobMachineInput::WireMembers {
                                edge: edge.clone(),
                            },
                            "resume_member_trust_repair",
                        )?;
                        resume_member_repair_authority_from_transition(
                            dsl_authority,
                            topology_epoch,
                            &transition,
                            edge,
                            &spec_peer_id,
                            "resume_member_trust_repair",
                        )?
                    }
                    ResumeTrustSource::External { key, edge } => {
                        let transition = apply_seeded_mob_input_collect_transition(
                            dsl_authority,
                            crate::machines::mob_machine::MobMachineInput::WireExternalPeer {
                                key: key.clone(),
                                edge: edge.clone(),
                            },
                            "resume_external_trust_repair",
                        )?;
                        resume_external_repair_authority_from_transition(
                            dsl_authority,
                            topology_epoch,
                            &transition,
                            edge,
                            &spec_peer_id,
                            "resume_external_trust_repair",
                        )?
                    }
                };
                resume_trust_mutations.push(ResumeTrustMutation {
                    comms: Arc::clone(&comms_a),
                    operation: ResumeTrustMutationOperation::Add(desired.spec.clone()),
                    authority: repair_authority,
                });
            }
        }
        preflight_resume_trust_mutations(&resume_trust_mutations, &mob_owner_token)
            .await
            .map_err(MobError::from)?;
        bind_resume_trust_mutation_owners(&resume_trust_mutations, &mob_owner_token)
            .await
            .map_err(MobError::from)?;
        for mutation in resume_trust_mutations {
            apply_resume_trust_mutation(mutation)
                .await
                .map_err(MobError::from)?;
        }
        // Notify orchestrator that the mob resumed.
        if notify_orchestrator_on_resume
            && let Some(orchestrator) = &definition.orchestrator
            && let Some(orchestrator_entry) = roster.by_profile(&orchestrator.profile).next()
        {
            if broken_members.contains(&orchestrator_entry.agent_identity) {
                tracing::warn!(
                    member_id = %orchestrator_entry.agent_identity,
                    "Skipping orchestrator resume notification because the orchestrator is Broken"
                );
                return Ok(());
            }
            let active_count = dsl_authority.state().identity_to_runtime.len();
            let wired_edges = dsl_authority.state().wiring_edges.len();
            let dsl_identity = crate::machines::mob_machine::AgentIdentity::from_domain(
                &orchestrator_entry.agent_identity,
            );
            let bridge_session_id = dsl_authority
                .state()
                .member_session_bindings
                .get(&dsl_identity)
                .and_then(|session_id| meerkat_core::types::SessionId::parse(&session_id.0).ok())
                .ok_or_else(|| {
                    MobError::Internal(
                        "orchestrator entry missing MobMachine session binding".to_string(),
                    )
                })?;
            let resume_message = format!(
                "Mob '{}' resumed with {} active meerkats and {} wiring links. Reconcile worker state and continue orchestration.",
                definition.id, active_count, wired_edges
            );
            match orchestrator_entry.runtime_mode {
                crate::MobRuntimeMode::AutonomousHost => {
                    let injector = session_service
                        .interaction_event_injector(&bridge_session_id)
                        .await
                        .ok_or_else(|| MobError::MissingMemberCapability {
                            member_id: orchestrator_entry.agent_identity.clone(),
                            capability: crate::error::MobMemberCapability::InteractionEventInjector,
                            context: "orchestrator resume notification",
                        })?;
                    injector
                        .inject(
                            resume_message.into(),
                            meerkat_core::PlainEventSource::Rpc,
                            meerkat_core::types::HandlingMode::Queue,
                            None,
                        )
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "orchestrator resume inject failed for '{}': {}",
                                orchestrator_entry.agent_identity, error
                            ))
                        })?;
                }
                crate::MobRuntimeMode::TurnDriven => {
                    provisioner
                        .start_turn(
                            &orchestrator_entry.member_ref,
                            meerkat_core::service::StartTurnRequest {
                                injected_context: Vec::new(),
                                prompt: resume_message.into(),
                                system_prompt: None,
                                event_tx: None,
                                runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(
                                ),
                            },
                        )
                        .await?;
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::too_many_arguments)]
    fn start_runtime(
        definition: Arc<MobDefinition>,
        initial_roster: Roster,
        mut dsl_authority: Box<crate::machines::mob_machine::MobMachineAuthority>,
        events: Arc<dyn MobEventStore>,
        run_store: Arc<dyn MobRunStore>,
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        supervisor_bridge: Arc<MobSupervisorBridge>,
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: RuntimeAdapterOption,
        workgraph_service: Option<meerkat::WorkGraphService>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        default_external_tools_provider: Option<crate::ExternalToolsProvider>,
        spawn_member_customizer: Option<Arc<dyn super::SpawnMemberCustomizer>>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
        realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
    ) -> Result<MobHandle, MobError> {
        // Row #320 seed: the orphan budget is machine state, seeded once from the
        // definition's `max_orphaned_turns` limit. Covers both the create and
        // resume runtime-start paths (both route through `start_runtime`).
        let max_orphaned_turns = definition
            .limits
            .as_ref()
            .and_then(|limits| limits.max_orphaned_turns)
            .unwrap_or(8) as u64;
        apply_seeded_mob_input(
            dsl_authority.as_mut(),
            crate::machines::mob_machine::MobMachineInput::SeedOrphanBudget {
                budget: max_orphaned_turns,
            },
            "seed_orphan_budget",
        )?;
        let (machine_state_watch_tx, _machine_state_watch_rx) =
            tokio::sync::watch::channel(dsl_authority.state().clone());
        let roster = Arc::new(RwLock::new(RosterAuthority::from_roster(initial_roster)));
        let (command_tx, command_rx) = mpsc::channel(MOB_COMMAND_CHANNEL_CAPACITY);
        let restore_diagnostics = Arc::new(RwLock::new(HashMap::new()));
        let wiring = RuntimeWiring {
            roster,
            dsl_authority,
            machine_state_watch_tx,
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
            command_tx,
            command_rx,
        };

        Ok(Self::start_runtime_with_components(
            definition,
            wiring,
            events,
            run_store,
            session_service,
            runtime_adapter,
            workgraph_service,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            spawn_member_customizer,
            realm_profile_store,
            BTreeMap::new(),
            realtime_session_factory,
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::too_many_arguments)]
    fn start_runtime_with_components(
        definition: Arc<MobDefinition>,
        wiring: RuntimeWiring,
        events: Arc<dyn MobEventStore>,
        run_store: Arc<dyn MobRunStore>,
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: RuntimeAdapterOption,
        workgraph_service: Option<meerkat::WorkGraphService>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        default_external_tools_provider: Option<crate::ExternalToolsProvider>,
        spawn_member_customizer: Option<Arc<dyn super::SpawnMemberCustomizer>>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
        per_spawn_external_tools: BTreeMap<AgentIdentity, Arc<dyn AgentToolDispatcher>>,
        realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
    ) -> MobHandle {
        let run_store = authority_validating_mob_run_store(run_store);
        let RuntimeWiring {
            roster,
            dsl_authority,
            machine_state_watch_tx,
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
            command_tx,
            command_rx,
        } = wiring;
        let external_backend = definition.backend.external.clone();
        let handle_session_service = session_service.clone();
        let wiring_public_phase = seeded_mob_public_phase(dsl_authority.state());
        // Terminal-phase watch: seed with the initial phase so a status()
        // call before any DSL transition returns the right answer.
        let (phase_watch_tx_actor, phase_watch_rx) =
            tokio::sync::watch::channel(wiring_public_phase);
        let handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster.clone(),
            definition: definition.clone(),
            events: events.clone(),
            run_store: run_store.clone(),
            flow_streams: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            session_service: handle_session_service.clone(),
            runtime_adapter: runtime_adapter.clone(),
            restore_diagnostics: restore_diagnostics.clone(),
            supervisor_bridge: supervisor_bridge.clone(),
            machine_state_watch_rx: machine_state_watch_tx.subscribe(),
            phase_watch_rx,
            realtime_session_factory,
        };
        let provisioner: Arc<dyn MobProvisioner> = Arc::new(
            MultiBackendProvisioner::new(
                session_service,
                runtime_adapter.clone(),
                workgraph_service,
                external_backend,
                supervisor_bridge.clone(),
            )
            .with_binding_persistence(definition.id.clone(), runtime_metadata.clone()),
        );
        // Row #320: the orphan budget is MobMachine state (seeded once in
        // `start_runtime` from `definition.limits.max_orphaned_turns`); the
        // executor no longer holds a shell-side budget.
        let flow_executor: Arc<dyn FlowTurnExecutor> = Arc::new(ActorFlowTurnExecutor::new(
            handle.clone(),
            provisioner.clone(),
        ));
        let topology_service = Arc::new(super::topology::MobTopologyService::new(
            definition.topology.clone(),
        ));
        // Normalize public phase: fresh creation + stale Creating restores both
        // become Running. Persisted Stopped/Completed/Destroyed are preserved.
        let public_phase = match wiring_public_phase {
            MobState::Creating => MobState::Running,
            phase => phase,
        };
        // Plain mobs (orchestrator: None in the definition) skip orchestrator
        // guards entirely. The coordinator-bound fact is MobMachine state
        // (see `coordinator_bound` in the mob DSL), initialized to `true`
        // by the machine's init block; no shell-side bind is needed.
        let has_orchestrator = definition.orchestrator.is_some();
        let flow_engine = FlowEngine::new(
            flow_executor,
            handle.clone(),
            run_store.clone(),
            events.clone(),
            topology_service,
        );
        let spawn_policy = Arc::new(super::spawn_policy::SpawnPolicyService::with_policy(
            super::spawn_policy::policy_from_definition_config(definition.spawn_policy.as_ref()),
        ));

        // Wave-c C-6c — flip the composition binding from `Standalone`
        // to `Wired(_)` whenever a runtime adapter is present, wiring
        // the mob producer into the `MeerkatConsumerSurface` on
        // `MeerkatMachine`. Builds without `runtime-adapter` keep the
        // standalone path (no consumer exists by construction).
        #[cfg(feature = "runtime-adapter")]
        let composition_binding = match &runtime_adapter {
            Some(adapter) => {
                let binding = super::composition::wired_binding_from_runtime_adapter(adapter);
                super::composition::attach_signal_dispatcher_to_runtime_adapter(
                    adapter,
                    command_tx.clone(),
                );
                binding
            }
            None => meerkat_runtime::composition::CompositionBinding::Standalone,
        };
        #[cfg(not(feature = "runtime-adapter"))]
        let composition_binding = meerkat_runtime::composition::CompositionBinding::Standalone;

        let dsl_topology_epoch = Arc::new(std::sync::atomic::AtomicU64::new(
            dsl_authority.state().topology_epoch,
        ));
        let dsl_authority_owner_token = dsl_authority.generated_authority_owner_token();

        let actor = MobActor {
            definition,
            roster,
            events,
            run_store,
            provisioner,
            flow_engine,
            has_orchestrator,
            run_tasks: BTreeMap::new(),
            run_cancel_tokens: BTreeMap::new(),
            flow_streams: handle.flow_streams.clone(),
            command_tx,
            tool_bundles,
            default_llm_client,
            retired_event_index: Arc::new(RwLock::new(HashSet::new())),
            autonomous_initial_turns: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            next_spawn_ticket: 0,
            next_fence_token: std::sync::atomic::AtomicU64::new(1),
            pending_spawns: PendingSpawnLineage::new(),
            pending_spawn_cleanup_anchors: BTreeMap::new(),
            edge_locks: Arc::new(super::edge_locks::EdgeLockRegistry::new()),
            lifecycle_tasks: tokio::task::JoinSet::new(),
            next_peer_delivery_ticket: 0,
            peer_delivery_tasks: tokio::task::JoinSet::new(),
            peer_delivery_inflight: BTreeMap::new(),
            peer_delivery_permits: Arc::new(tokio::sync::Semaphore::new(
                super::actor::MAX_PENDING_PEER_DELIVERIES,
            )),
            session_service: handle_session_service,
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter,
            restore_diagnostics,
            member_revival_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            runtime_metadata,
            supervisor_bridge,
            spawn_policy,
            dsl_authority: *dsl_authority,
            dsl_topology_epoch,
            dsl_authority_owner_token,
            machine_state_watch_tx,
            phase_watch_tx: phase_watch_tx_actor,
            default_external_tools_provider,
            per_spawn_external_tools: tokio::sync::RwLock::new(per_spawn_external_tools),
            spawn_member_customizer,
            realm_profile_store,
            composition_binding,
            pending_routed_effects: Vec::new(),
            destroy_cleanup_active: false,
        };
        tokio::spawn(actor.run(command_rx));

        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{MemberSpawnedEvent, MobEvent, MobEventKind};
    use crate::ids::Generation;
    use crate::roster::{MobMemberKickoffPhase, MobMemberKickoffSnapshot};
    use chrono::Utc;
    use meerkat_core::time_compat::UNIX_EPOCH;

    #[test]
    fn seeded_authority_recovers_kickoff_lifecycle() {
        let definition = MobDefinition::explicit("test-mob");
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let events = vec![
            MobEvent {
                cursor: 1,
                timestamp: Utc::now(),
                mob_id: definition.id.clone(),
                kind: MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                    identity.clone(),
                    Generation::INITIAL,
                    FenceToken::new(1),
                    runtime_id,
                    ProfileName::from("worker"),
                )),
            },
            MobEvent {
                cursor: 2,
                timestamp: Utc::now(),
                mob_id: definition.id.clone(),
                kind: MobEventKind::MemberKickoffUpdated {
                    member: identity.clone(),
                    kickoff: MobMemberKickoffSnapshot {
                        phase: MobMemberKickoffPhase::Pending,
                        error: None,
                        updated_at: UNIX_EPOCH,
                    },
                },
            },
        ];
        let mut authority = seed_mob_authority();

        seed_mob_authority_sync_from_events(&mut authority, &events, &definition)
            .expect("durable kickoff replay should seed MobMachine");

        assert!(authority.state().member_kickoff_pending.contains(
            &crate::machines::mob_machine::AgentIdentity::from_domain(&identity,)
        ));
    }

    #[test]
    fn recovered_retiring_runtime_suppresses_trust_repair_without_erasing_topology() {
        let definition = MobDefinition::explicit("retiring-trust-replay");
        let retiring = AgentIdentity::from("alpha");
        let active_peer = AgentIdentity::from("beta");
        let unrelated_active = AgentIdentity::from("gamma");
        let retiring_runtime = AgentRuntimeId::initial(retiring.clone());
        let active_peer_runtime = AgentRuntimeId::initial(active_peer.clone());
        let unrelated_runtime = AgentRuntimeId::initial(unrelated_active.clone());
        let retiring_pubkey = [11; 32];
        let retiring_peer_id = PeerId::from_ed25519_pubkey(&retiring_pubkey);
        let retiring_endpoint = TrustedPeerDescriptor::unsigned_with_pubkey(
            "retiring-trust-replay/worker/alpha",
            retiring_peer_id.to_string(),
            retiring_pubkey,
            "inproc://retiring-trust-replay/worker/alpha",
        )
        .expect("retiring endpoint");
        let external_pubkey = [12; 32];
        let external_peer_id = PeerId::from_ed25519_pubkey(&external_pubkey);
        let external_endpoint = TrustedPeerDescriptor::unsigned_with_pubkey(
            "external-peer",
            external_peer_id.to_string(),
            external_pubkey,
            "tcp://127.0.0.1:32123",
        )
        .expect("external endpoint");
        let event = |cursor, kind| MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: definition.id.clone(),
            kind,
        };
        let retiring_edge = dsl_wiring_edge(&retiring, &active_peer);
        let active_edge = dsl_wiring_edge(&active_peer, &unrelated_active);
        let events = vec![
            event(
                1,
                MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                    retiring.clone(),
                    Generation::INITIAL,
                    FenceToken::new(1),
                    retiring_runtime.clone(),
                    ProfileName::from("worker"),
                )),
            ),
            event(
                2,
                MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                    active_peer.clone(),
                    Generation::INITIAL,
                    FenceToken::new(1),
                    active_peer_runtime,
                    ProfileName::from("worker"),
                )),
            ),
            event(
                3,
                MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                    unrelated_active.clone(),
                    Generation::INITIAL,
                    FenceToken::new(1),
                    unrelated_runtime,
                    ProfileName::from("worker"),
                )),
            ),
            event(
                4,
                MobEventKind::MembersWired {
                    a: retiring.clone(),
                    b: active_peer.clone(),
                },
            ),
            event(
                5,
                MobEventKind::MembersWired {
                    a: active_peer.clone(),
                    b: unrelated_active,
                },
            ),
            event(
                6,
                MobEventKind::ExternalPeerWired {
                    local: retiring.clone(),
                    spec: external_endpoint.clone(),
                },
            ),
            event(7, MobEventKind::MobDestroying),
            event(
                8,
                MobEventKind::MemberRetirementStarted {
                    agent_identity: retiring.clone(),
                    agent_runtime_id: retiring_runtime.clone(),
                    generation: Generation::INITIAL,
                    role: ProfileName::from("worker"),
                    releasing: None,
                    session_id: None,
                    retiring_peer_endpoint: Some(retiring_endpoint.clone()),
                },
            ),
            event(
                9,
                MobEventKind::RemoteMemberRuntimeRetired {
                    agent_identity: retiring.clone(),
                    agent_runtime_id: retiring_runtime.clone(),
                    fence_token: FenceToken::new(1),
                    generation: Generation::INITIAL,
                },
            ),
            event(
                10,
                MobEventKind::RemoteMemberSupervisorRevoked {
                    agent_identity: retiring.clone(),
                    agent_runtime_id: retiring_runtime.clone(),
                    fence_token: FenceToken::new(1),
                    generation: Generation::INITIAL,
                },
            ),
        ];
        let mut authority = seed_mob_authority();

        seed_mob_authority_sync_from_events(&mut authority, &events, &definition)
            .expect("durable retirement replay should seed MobMachine");

        let retiring_dsl = crate::machines::mob_machine::AgentIdentity::from_domain(&retiring);
        let active_peer_dsl =
            crate::machines::mob_machine::AgentIdentity::from_domain(&active_peer);
        let external_edge = crate::machines::mob_machine::ExternalPeerEdge::new(
            retiring_dsl.clone(),
            crate::machines::mob_machine::ExternalPeerEndpoint::from(&external_endpoint),
        );
        let external_key = dsl_external_peer_key(&retiring, &external_endpoint.name);
        {
            let state = authority.state();
            assert!(state.wiring_edges.contains(&retiring_edge));
            assert!(state.wiring_edges.contains(&active_edge));
            assert!(state.external_peer_edges.contains(&external_edge));
            assert!(recovered_endpoint_runtime_is_retiring(state, &retiring_dsl));
            assert!(state.remote_runtime_retired_exact(
                &retiring_dsl,
                &crate::machines::mob_machine::AgentRuntimeId::from_domain(&retiring_runtime),
                crate::machines::mob_machine::FenceToken::from_domain(FenceToken::new(1)),
                crate::machines::mob_machine::Generation::from_domain(Generation::INITIAL),
            ));
            assert!(state.remote_supervisor_revoked_exact(
                &retiring_dsl,
                &crate::machines::mob_machine::AgentRuntimeId::from_domain(&retiring_runtime),
                crate::machines::mob_machine::FenceToken::from_domain(FenceToken::new(1)),
                crate::machines::mob_machine::Generation::from_domain(Generation::INITIAL),
            ));
            assert!(!recovered_endpoint_runtime_is_retiring(
                state,
                &active_peer_dsl
            ));
            assert_eq!(
                state.member_peer_endpoints.get(&retiring_dsl),
                Some(&crate::machines::mob_machine::MemberPeerEndpoint::from(
                    &retiring_endpoint,
                )),
                "retirement replay must retain the exact endpoint used by surviving trust rows"
            );
            assert!(!recovered_member_edge_allows_trust_repair(
                state,
                &retiring_edge
            ));
            assert!(recovered_member_edge_allows_trust_repair(
                state,
                &active_edge
            ));
            assert!(!recovered_peer_only_overlay_allows_trust_reconcile(
                state,
                &state.wiring_edges,
                &active_peer_dsl,
            ));
        }

        let remote_checkpoint = apply_seeded_mob_input_transition(
            &mut authority,
            crate::machines::mob_machine::MobMachineInput::RecordRemoteMemberRuntimeRetired {
                agent_identity: retiring_dsl.clone(),
                agent_runtime_id: crate::machines::mob_machine::AgentRuntimeId::from_domain(
                    &retiring_runtime,
                ),
                fence_token: crate::machines::mob_machine::FenceToken::from_domain(
                    FenceToken::new(1),
                ),
                generation: crate::machines::mob_machine::Generation::from_domain(
                    Generation::INITIAL,
                ),
            },
            "test_recovered_remote_runtime_checkpoint_idempotent",
        )
        .expect("exact remote runtime checkpoint should be idempotent after replay");
        assert!(remote_checkpoint.effects().iter().any(|effect| matches!(
            effect,
            crate::machines::mob_machine::MobMachineEffect::AppendLifecycleJournal {
                kind: crate::machines::mob_machine::MobLifecycleJournalKind::RemoteMemberRuntimeRetired,
                ..
            }
        )));

        let supervisor_checkpoint = apply_seeded_mob_input_transition(
            &mut authority,
            crate::machines::mob_machine::MobMachineInput::RecordRemoteMemberSupervisorRevoked {
                agent_identity: retiring_dsl.clone(),
                agent_runtime_id: crate::machines::mob_machine::AgentRuntimeId::from_domain(
                    &retiring_runtime,
                ),
                fence_token: crate::machines::mob_machine::FenceToken::from_domain(
                    FenceToken::new(1),
                ),
                generation: crate::machines::mob_machine::Generation::from_domain(
                    Generation::INITIAL,
                ),
            },
            "test_recovered_remote_supervisor_checkpoint_idempotent",
        )
        .expect("exact remote supervisor checkpoint should be idempotent after replay");
        assert!(supervisor_checkpoint.effects().iter().any(|effect| matches!(
            effect,
            crate::machines::mob_machine::MobMachineEffect::AppendLifecycleJournal {
                kind: crate::machines::mob_machine::MobLifecycleJournalKind::RemoteMemberSupervisorRevoked,
                ..
            }
        )));

        let stale_observation = apply_seeded_mob_input_transition(
            &mut authority,
            crate::machines::mob_machine::MobMachineInput::AuthorizeRetiringMemberTrustCleanupObserved {
                edge: retiring_edge.clone(),
                a_identity: retiring_edge.a.clone(),
                a_peer_id: crate::machines::mob_machine::PeerId::from("alpha-peer"),
                b_identity: retiring_edge.b.clone(),
                b_peer_id: crate::machines::mob_machine::PeerId::from("beta-peer"),
                agent_identity: retiring_dsl.clone(),
                agent_runtime_id: crate::machines::mob_machine::AgentRuntimeId::from_domain(
                    &retiring_runtime,
                ),
                fence_token: crate::machines::mob_machine::FenceToken::from_domain(
                    FenceToken::new(2),
                ),
                generation: crate::machines::mob_machine::Generation::from_domain(
                    Generation::INITIAL,
                ),
            },
            "test_recovered_retiring_cleanup_stale_fence",
        );
        assert!(
            stale_observation.is_err(),
            "stale retirement material must not authorize observed trust cleanup"
        );

        let transition = apply_seeded_mob_input_transition(
            &mut authority,
            crate::machines::mob_machine::MobMachineInput::AuthorizeRetiringMemberTrustCleanupObserved {
                edge: retiring_edge.clone(),
                a_identity: retiring_edge.a.clone(),
                a_peer_id: crate::machines::mob_machine::PeerId::from("alpha-peer"),
                b_identity: retiring_edge.b.clone(),
                b_peer_id: crate::machines::mob_machine::PeerId::from("beta-peer"),
                agent_identity: retiring_dsl,
                agent_runtime_id: crate::machines::mob_machine::AgentRuntimeId::from_domain(
                    &retiring_runtime,
                ),
                fence_token: crate::machines::mob_machine::FenceToken::from_domain(
                    FenceToken::new(1),
                ),
                generation: crate::machines::mob_machine::Generation::from_domain(
                    Generation::INITIAL,
                ),
            },
            "test_recovered_retiring_cleanup_exact_runtime",
        )
        .expect("exact replayed retirement material should authorize observed trust cleanup");
        assert!(transition.effects().iter().any(|effect| matches!(
            effect,
            crate::machines::mob_machine::MobMachineEffect::MemberTrustUnwiringRequested {
                edge,
                a_peer_id,
                b_peer_id,
                ..
            } if edge == &retiring_edge
                && a_peer_id.0 == "alpha-peer"
                && b_peer_id.0 == "beta-peer"
        )));

        let external_cleanup = apply_seeded_mob_input_transition(
            &mut authority,
            crate::machines::mob_machine::MobMachineInput::CleanupRetiringExternalPeerObservedAbsent {
                key: external_key,
                edge: external_edge.clone(),
                agent_identity: crate::machines::mob_machine::AgentIdentity::from_domain(&retiring),
                agent_runtime_id: crate::machines::mob_machine::AgentRuntimeId::from_domain(
                    &retiring_runtime,
                ),
                fence_token: crate::machines::mob_machine::FenceToken::from_domain(
                    FenceToken::new(1),
                ),
                generation: crate::machines::mob_machine::Generation::from_domain(
                    Generation::INITIAL,
                ),
            },
            "test_recovered_retiring_external_cleanup_without_runtime",
        )
        .expect("exact replayed endpoint should authorize no-effect external cleanup without live comms");
        assert!(
            !authority
                .state()
                .external_peer_edges
                .contains(&external_edge)
        );
        assert!(external_cleanup.effects().iter().any(|effect| matches!(
            effect,
            crate::machines::mob_machine::MobMachineEffect::WiringGraphChanged { .. }
        )));
        assert!(external_cleanup.effects().iter().all(|effect| !matches!(
            effect,
            crate::machines::mob_machine::MobMachineEffect::ExternalPeerTrustUnwiringRequested { .. }
        )));

        let generic_terminal = apply_seeded_mob_signal_transition(
            &mut authority,
            crate::machines::mob_machine::MobMachineSignal::ObserveMemberRetirementArchived {
                agent_identity: crate::machines::mob_machine::AgentIdentity::from_domain(&retiring),
                agent_runtime_id: crate::machines::mob_machine::AgentRuntimeId::from_domain(
                    &retiring_runtime,
                ),
                fence_token: crate::machines::mob_machine::FenceToken::from_domain(
                    FenceToken::new(1),
                ),
                generation: crate::machines::mob_machine::Generation::from_domain(
                    Generation::INITIAL,
                ),
                session_id: None,
            },
            "test_remote_runtime_generic_terminal_rejected",
        );
        assert!(
            generic_terminal.is_err(),
            "generic session archive observation must not terminalize a peer-only runtime"
        );

        apply_seeded_mob_signal(
            &mut authority,
            crate::machines::mob_machine::MobMachineSignal::ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked {
                agent_identity: crate::machines::mob_machine::AgentIdentity::from_domain(&retiring),
                agent_runtime_id: crate::machines::mob_machine::AgentRuntimeId::from_domain(
                    &retiring_runtime,
                ),
                fence_token: crate::machines::mob_machine::FenceToken::from_domain(
                    FenceToken::new(1),
                ),
                generation: crate::machines::mob_machine::Generation::from_domain(
                    Generation::INITIAL,
                ),
            },
            "test_recovered_remote_runtime_revoked_terminal_clear",
        )
        .expect("revoked remote terminal proof should clear the runtime checkpoint");
        assert!(authority.state().remote_runtime_retired_ids.is_empty());
    }
}
