use super::*;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::comms::{
    CommsTrustMutation, CommsTrustMutationAuthority, CommsTrustMutationResult, SendError,
    TrustedPeerDescriptor,
};
use meerkat_core::generated::comms_trust_authority;
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::collections::HashSet;

const MOB_COMMAND_CHANNEL_CAPACITY: usize = 4096;

struct ResumeDesiredTrust {
    spec: TrustedPeerDescriptor,
    source: ResumeTrustSource,
}

enum ResumeTrustSource {
    Member(crate::machines::mob_machine::WiringEdge),
    External {
        key: crate::machines::mob_machine::ExternalPeerKey,
        edge: crate::machines::mob_machine::ExternalPeerEdge,
    },
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
    allow_ephemeral_sessions: bool,
    notify_orchestrator_on_resume: bool,
    tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    default_external_tools_provider: Option<crate::ExternalToolsProvider>,
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

fn seed_mob_authority() -> crate::machines::mob_machine::MobMachineAuthority {
    crate::machines::mob_machine::MobMachineAuthority::new()
}

fn apply_seeded_mob_input(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    input: crate::machines::mob_machine::MobMachineInput,
    context: &'static str,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let input_debug = format!("{input:?}");
    let transition = mob_dsl::MobMachineMutator::apply(authority, input).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine seeded authority ({context}) rejected {input_debug}: {error}"
        ))
    })?;
    let _ = transition;
    Ok(())
}

fn register_seeded_member_peer(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    agent_identity: &crate::ids::AgentIdentity,
    peer_id: &str,
    context: &'static str,
) -> Result<(), MobError> {
    apply_seeded_mob_input(
        authority,
        crate::machines::mob_machine::MobMachineInput::RegisterMemberPeer {
            agent_identity: crate::machines::mob_machine::AgentIdentity::from_domain(
                agent_identity,
            ),
            peer_id: crate::machines::mob_machine::PeerId::from(peer_id.to_string()),
        },
        context,
    )
}

fn apply_seeded_mob_input_collect_effects(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    input: crate::machines::mob_machine::MobMachineInput,
    context: &'static str,
) -> Result<Vec<crate::machines::mob_machine::MobMachineEffect>, MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let input_debug = format!("{input:?}");
    let transition = mob_dsl::MobMachineMutator::apply(authority, input).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine seeded authority ({context}) rejected {input_debug}: {error}"
        ))
    })?;
    Ok(transition.effects)
}

fn apply_seeded_mob_signal(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    signal: crate::machines::mob_machine::MobMachineSignal,
    context: &'static str,
) -> Result<(), MobError> {
    let signal_debug = format!("{signal:?}");
    let transition = authority.apply_signal(signal).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine seeded authority ({context}) rejected {signal_debug}: {error}"
        ))
    })?;
    let _ = transition;
    Ok(())
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

fn resume_member_repair_authority_from_effects(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    effects: &[crate::machines::mob_machine::MobMachineEffect],
    edge: &crate::machines::mob_machine::WiringEdge,
    peer_id: &str,
    context: &'static str,
) -> Result<CommsTrustMutationAuthority, MobError> {
    let graph_changed = seeded_effects_include_wiring_graph_change(effects);
    let repair_requested = effects.iter().any(|effect| {
        matches!(
            effect,
            crate::machines::mob_machine::MobMachineEffect::WiringTrustRepairRequested {
                edge: effect_edge
            } if effect_edge == edge
        )
    });
    if !repair_requested || graph_changed {
        return Err(MobError::WiringError(format!(
            "{context} produced no generated member trust repair authority for peer '{peer_id}'"
        )));
    }

    let handoff_effects = apply_seeded_mob_input_collect_effects(
        authority,
        crate::machines::mob_machine::MobMachineInput::AuthorizeMemberTrustWiring {
            edge: edge.clone(),
            a_identity: edge.a.clone(),
            b_identity: edge.b.clone(),
        },
        context,
    )?;
    handoff_effects
        .iter()
        .find_map(|effect| match effect {
            crate::machines::mob_machine::MobMachineEffect::MemberTrustWiringRequested {
                edge: effect_edge,
                a_peer_id,
                b_peer_id,
                epoch,
            } if effect_edge == edge && (a_peer_id.0 == peer_id || b_peer_id.0 == peer_id) => Some(
                comms_trust_authority::mob_machine_peer_repair(peer_id, *epoch),
            ),
            _ => None,
        })
        .ok_or_else(|| {
            MobError::WiringError(format!(
                "{context} produced no generated member trust handoff for peer '{peer_id}'"
            ))
        })
}

fn resume_external_repair_authority_from_effects(
    effects: &[crate::machines::mob_machine::MobMachineEffect],
    edge: &crate::machines::mob_machine::ExternalPeerEdge,
    peer_id: &str,
    epoch: u64,
    context: &'static str,
) -> Result<CommsTrustMutationAuthority, MobError> {
    let graph_changed = seeded_effects_include_wiring_graph_change(effects);
    let repair_requested = effects.iter().any(|effect| {
        matches!(
            effect,
            crate::machines::mob_machine::MobMachineEffect::ExternalPeerTrustRepairRequested {
                edge: effect_edge
            } if effect_edge == edge
        )
    });
    if repair_requested && !graph_changed {
        return Ok(comms_trust_authority::mob_machine_peer_repair(
            peer_id, epoch,
        ));
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

async fn apply_resume_trusted_peer_add(
    comms: &(dyn CoreCommsRuntime + '_),
    peer: TrustedPeerDescriptor,
    authority: CommsTrustMutationAuthority,
) -> Result<(), SendError> {
    match comms
        .apply_trust_mutation(CommsTrustMutation::AddTrustedPeer { peer, authority })
        .await?
    {
        CommsTrustMutationResult::Added => Ok(()),
        result => Err(unexpected_resume_trust_mutation_result(
            "resume add trusted peer",
            result,
        )),
    }
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
                    },
                    "recover_member_reset",
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
            MobEventKind::MemberKickoffUpdated { .. } => {}
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
    Ok(())
}

async fn seed_mob_authority_sync_from_flow_runs(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    event_store: Arc<dyn crate::store::MobEventStore>,
    mob_id: &MobId,
) -> Result<(), MobError> {
    use crate::run::MobRun;

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
        if run.status.is_terminal() || run.flow_authority_inputs.is_empty() {
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
    let transitioned = if before_terminal.status.is_terminal() {
        false
    } else {
        commit_recovered_flow_run_command(
            authority,
            run_store.clone(),
            &run_id,
            MobMachineFlowRunCommand::TerminalizeCanceled(flow_run::inputs::TerminalizeCanceled {}),
            Some(MobRunStatus::Canceled),
            "resume_recovered_flow_terminalize_canceled",
        )
        .await?
    };
    if transitioned {
        terminalization
            .record_persisted_terminalization(
                run_id.clone(),
                before_terminal.flow_id,
                super::terminalization::TerminalizationTarget::Canceled,
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
        if run.status.is_terminal() {
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
            &transition.effects,
        )?;

        let won = if let Some(next_status) = &next_status {
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
        } else {
            run_store
                .cas_flow_state_with_authority(
                    run_id,
                    &run.flow_state,
                    &outcome.next_state,
                    vec![authority_input.clone()],
                )
                .await?
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
    restore_diagnostics: &HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic>,
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
    restore_diagnostics: Arc<RwLock<HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic>>>,
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
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
            default_external_tools_provider: None,
            realtime_session_factory: None,
        }
    }

    /// Create a builder from a mobpack definition + packed skill files.
    ///
    /// Any `SkillSource::Path` entries are resolved against `packed_skills` and
    /// converted to inline sources for runtime execution.
    pub fn from_mobpack(
        mut definition: MobDefinition,
        packed_skills: BTreeMap<String, Vec<u8>>,
        storage: MobStorage,
    ) -> Result<Self, MobError> {
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
        Ok(Self::new(definition, storage))
    }

    /// Create a builder that resumes a mob from persisted events.
    pub fn for_resume(storage: MobStorage) -> Self {
        Self {
            mode: BuilderMode::Resume,
            storage,
            session_service: None,
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: None,
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
            default_external_tools_provider: None,
            realtime_session_factory: None,
        }
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
            allow_ephemeral_sessions,
            notify_orchestrator_on_resume: _,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
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
        Self::sync_definition_with_spec_store(
            storage.specs.clone(),
            definition.id.clone(),
            definition.as_ref(),
        )
        .await?;
        Self::ensure_supervisor_authority(storage.runtime_metadata.clone(), definition.id.clone())
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
        let supervisor_bridge = Arc::new(MobSupervisorBridge::new(
            &definition.id,
            supervisor_authority,
        )?);

        Self::start_runtime(
            definition,
            Roster::new(),
            MobState::Running,
            storage.events.clone(),
            storage.runs.clone(),
            storage.runtime_metadata.clone(),
            supervisor_bridge,
            session_service,
            runtime_adapter,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
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
            allow_ephemeral_sessions,
            notify_orchestrator_on_resume,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
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
        seed_mob_authority_sync_from_events(&mut initial_dsl_authority, epoch_events, &definition)?;
        let resumed_state = seeded_mob_public_phase(initial_dsl_authority.state());
        // Runtime metadata owns supervisor authority. External-binding
        // overlays are compatibility projections only.
        if !destroy_storage_finalizing {
            Self::ensure_supervisor_authority(
                storage.runtime_metadata.clone(),
                definition.id.clone(),
            )
            .await?;
        }
        let supervisor_authority = match storage
            .runtime_metadata
            .load_supervisor_authority(&definition.id)
            .await?
        {
            Some(record) if record.protocol_version.is_supported() => record,
            Some(record) => {
                return Err(MobError::WiringError(format!(
                    "unsupported supervisor bridge protocol version {} (supported {:?}; default {})",
                    record.protocol_version,
                    super::bridge_protocol::supervisor_bridge_supported_protocol_versions(),
                    super::bridge_protocol::supervisor_bridge_default_protocol_version()
                )));
            }
            None if destroy_storage_finalizing => {
                crate::store::SupervisorAuthorityRecord::generate(
                    super::bridge_protocol::supervisor_bridge_default_protocol_version(),
                )
            }
            None => {
                return Err(MobError::Internal(format!(
                    "cannot resume mob '{}': missing supervisor runtime metadata",
                    definition.id
                )));
            }
        };
        let supervisor_bridge = Arc::new(MobSupervisorBridge::new(
            &definition.id,
            supervisor_authority,
        )?);
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
            machine_state_watch_rx,
            phase_watch_rx: preview_phase_rx,
            realtime_session_factory: realtime_session_factory.clone(),
        };
        // session_service is still live here (not consumed until start_runtime_with_components)

        if resumed_state == MobState::Running {
            Self::reconcile_resume(
                &definition,
                &mut roster,
                &session_service,
                runtime_adapter.clone(),
                supervisor_bridge.clone(),
                notify_orchestrator_on_resume,
                default_llm_client.clone(),
                &tool_bundles,
                wiring.dsl_authority.as_mut(),
                &preview_handle,
                &default_external_tools_provider,
                storage.realm_profiles.clone(),
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
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            storage.realm_profiles.clone(),
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
    ) -> Result<(), MobError> {
        let default_protocol_version =
            super::bridge_protocol::supervisor_bridge_default_protocol_version();
        match runtime_metadata.load_supervisor_authority(&mob_id).await? {
            None => {
                let record =
                    crate::store::SupervisorAuthorityRecord::generate(default_protocol_version);
                runtime_metadata
                    .put_supervisor_authority_if_absent(&mob_id, &record)
                    .await?;
            }
            Some(record) if record.protocol_version.is_supported() => {}
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
        supervisor_bridge: Arc<MobSupervisorBridge>,
        notify_orchestrator_on_resume: bool,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        dsl_authority: &mut crate::machines::mob_machine::MobMachineAuthority,
        tool_handle: &MobHandle,
        default_external_tools_provider: &Option<crate::ExternalToolsProvider>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
    ) -> Result<(), MobError> {
        let provisioner = MultiBackendProvisioner::new(
            session_service.clone(),
            runtime_adapter.clone(),
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
            let Some(bridge_session_id) = dsl_authority
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
            let record_restore_failure = |reason: String| async {
                tool_handle.restore_diagnostics.write().await.insert(
                    entry.agent_identity.clone(),
                    super::handle::RestoreFailureDiagnostic {
                        bridge_session_id: Some(bridge_session_id.clone()),
                        reason,
                    },
                );
            };

            if matches!(entry.member_ref, MemberRef::Session { .. })
                && session_service.supports_persistent_sessions()
            {
                let Some(stored_session) = session_service
                    .load_persisted_session(&bridge_session_id)
                    .await?
                else {
                    record_restore_failure(format!(
                        "missing durable session snapshot for '{bridge_session_id}'"
                    ))
                    .await;
                    continue;
                };
                // Prefer roster's effective_profile_override on restore for lifecycle safety.
                let profile = if let Some(ref p) = entry.effective_profile_override {
                    p.clone()
                } else {
                    definition
                        .resolve_profile(&entry.role, realm_profile_store.as_ref())
                        .await?
                };
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
                                None,
                            )?,
                            context: None,
                            labels: Some(entry.labels.clone()),
                            additional_instructions: None,
                            shell_env: None,
                            mob_tool_authority_context: None,
                            inherited_tool_filter: None,
                        },
                        expected_session_id: &bridge_session_id,
                        resumed_session: stored_session,
                    })
                    .await;
                let mut resumed_config = match resumed_config {
                    Ok(config) => config,
                    Err(error) => {
                        record_restore_failure(error.to_string()).await;
                        continue;
                    }
                };
                resumed_config.keep_alive =
                    entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
                let reconcile_client: Arc<dyn LlmClient> = default_llm_client
                    .clone()
                    .unwrap_or_else(|| Arc::new(meerkat_client::TestClient::default()));
                resumed_config.llm_client_override = Some(reconcile_client);
                let prompt = format!(
                    "You have been spawned as '{}' (role: {}) in mob '{}'.",
                    entry.agent_identity, entry.role, definition.id
                );
                let req = build::to_create_session_request(&resumed_config, prompt.into());
                match session_service.create_session(req).await {
                    Ok(created) => {
                        let created_bridge_session_id = created.session_id;
                        if let Err(error) = apply_seeded_member_session_binding(
                            dsl_authority,
                            &entry.agent_identity,
                            &entry.agent_runtime_id,
                            &created_bridge_session_id,
                            "resume_recovered_member_session_binding",
                        ) {
                            record_restore_failure(format!(
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
                        record_restore_failure(format!(
                            "failed to restore durable session '{bridge_session_id}': {error}"
                        ))
                        .await;
                    }
                }
                continue;
                // Ephemeral services can still fall back to fresh-create.
            }
            let profile = definition
                .resolve_profile(&entry.role, realm_profile_store.as_ref())
                .await?;
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
                    None,
                )?,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
                inherited_tool_filter: None,
            })
            .await?;
            config.keep_alive = entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
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
            let created = session_service.create_session(req).await?;
            let created_bridge_session_id = created.session_id;
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
        let entries = roster.list().cloned().collect::<Vec<_>>();
        let machine_wiring_edges = dsl_authority.state().wiring_edges.clone();
        let machine_external_peer_edges = dsl_authority.state().external_peer_edges.clone();
        let broken_members = tool_handle
            .restore_diagnostics
            .read()
            .await
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        for entry in &entries {
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
                register_seeded_member_peer(
                    dsl_authority,
                    &entry.agent_identity,
                    &peer_id_a.to_string(),
                    "resume_register_member_peer",
                )?;
            } else if let Some(peer_id) = entry.peer_id() {
                register_seeded_member_peer(
                    dsl_authority,
                    &entry.agent_identity,
                    &peer_id.to_string(),
                    "resume_register_backend_member_peer",
                )?;
            }
        }
        for entry in &entries {
            if broken_members.contains(&entry.agent_identity) {
                continue;
            }
            let local_comms = provisioner.comms_runtime(&entry.member_ref).await;
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
                let peer_identity = AgentIdentity::from(peer_dsl_identity.0.as_str());
                let peer_meerkat_id = MeerkatId::from(peer_identity.as_str());
                let peer_entry = roster.get(&peer_meerkat_id).cloned().ok_or_else(|| {
                    MobError::WiringError(format!(
                        "resume machine wiring target '{}' missing for '{}'",
                        peer_identity, entry.agent_identity
                    ))
                })?;
                if broken_members.contains(&peer_meerkat_id) {
                    continue;
                }
                let name_b = format!(
                    "{}/{}/{}",
                    definition.id, peer_entry.role, peer_entry.agent_identity
                );
                let fallback_peer_id = match provisioner.comms_runtime(&peer_entry.member_ref).await
                {
                    Some(comms_b) => comms_b.public_key().ok_or_else(|| {
                        MobError::WiringError(format!(
                            "resume requires public key for '{}' -> '{}'",
                            entry.agent_identity, peer_identity
                        ))
                    })?,
                    None => match &peer_entry.member_ref {
                        crate::event::MemberRef::BackendPeer {
                            peer_id,
                            session_id: None,
                            ..
                        } => peer_id.clone(),
                        _ => {
                            return Err(MobError::WiringError(format!(
                                "resume requires comms runtime for '{}' -> '{}'",
                                entry.agent_identity, peer_identity
                            )));
                        }
                    },
                };
                let spec = provisioner
                    .trusted_peer_spec(&peer_entry.member_ref, &name_b, &fallback_peer_id)
                    .await?;
                desired_trust.push(ResumeDesiredTrust {
                    spec,
                    source: ResumeTrustSource::Member(edge.clone()),
                });
            }

            for edge in &machine_external_peer_edges {
                if edge.local == local_dsl_identity {
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
                // Peer-only external members have no local comms runtime on
                // the supervisor side; their trust lives on the remote
                // process, so resume reconciles it through the supervisor
                // bridge instead of mutating local state.
                let desired_specs = desired_trust
                    .iter()
                    .map(|desired| desired.spec.clone())
                    .collect::<Vec<_>>();
                provisioner
                    .reconcile_peer_only_trust(&entry.member_ref, &desired_specs)
                    .await?;
                continue;
            };
            let current_peers = comms_a.peers().await;
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
                        let effects = apply_seeded_mob_input_collect_effects(
                            dsl_authority,
                            crate::machines::mob_machine::MobMachineInput::WireMembers {
                                edge: edge.clone(),
                            },
                            "resume_member_trust_repair",
                        )?;
                        resume_member_repair_authority_from_effects(
                            dsl_authority,
                            &effects,
                            edge,
                            &spec_peer_id,
                            "resume_member_trust_repair",
                        )?
                    }
                    ResumeTrustSource::External { key, edge } => {
                        let effects = apply_seeded_mob_input_collect_effects(
                            dsl_authority,
                            crate::machines::mob_machine::MobMachineInput::WireExternalPeer {
                                key: key.clone(),
                                edge: edge.clone(),
                            },
                            "resume_external_trust_repair",
                        )?;
                        resume_external_repair_authority_from_effects(
                            &effects,
                            edge,
                            &spec_peer_id,
                            dsl_authority.state().topology_epoch,
                            "resume_external_trust_repair",
                        )?
                    }
                };
                if let Err(error) = apply_resume_trusted_peer_add(
                    comms_a.as_ref(),
                    desired.spec.clone(),
                    repair_authority,
                )
                .await
                {
                    tracing::warn!(
                        agent_identity = %entry.agent_identity,
                        peer = %desired.spec.name,
                        %error,
                        "resume: failed to re-establish trust"
                    );
                }
            }
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
            let active_count = roster.len();
            let wired_edges = roster
                .list()
                .map(|entry| entry.wired_to.len())
                .sum::<usize>()
                / 2;
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
        initial_state: MobState,
        events: Arc<dyn MobEventStore>,
        run_store: Arc<dyn MobRunStore>,
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        supervisor_bridge: Arc<MobSupervisorBridge>,
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: RuntimeAdapterOption,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        default_external_tools_provider: Option<crate::ExternalToolsProvider>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
        realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
    ) -> Result<MobHandle, MobError> {
        let mut dsl_authority = Box::new(seed_mob_authority());
        finish_seeded_mob_authority_phase(&mut dsl_authority, initial_state)?;
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
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            realm_profile_store,
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
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        default_external_tools_provider: Option<crate::ExternalToolsProvider>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
        realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
    ) -> MobHandle {
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
        let pending_supervisor_rotation_fallback = Arc::new(tokio::sync::RwLock::new(None));
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
            machine_state_watch_rx: machine_state_watch_tx.subscribe(),
            phase_watch_rx,
            realtime_session_factory,
        };
        let provisioner: Arc<dyn MobProvisioner> = Arc::new(
            MultiBackendProvisioner::new(
                session_service,
                runtime_adapter.clone(),
                external_backend,
                supervisor_bridge.clone(),
            )
            .with_binding_persistence(
                definition.id.clone(),
                runtime_metadata.clone(),
                roster.clone(),
                pending_supervisor_rotation_fallback.clone(),
            ),
        );
        let max_orphaned_turns = definition
            .limits
            .as_ref()
            .and_then(|limits| limits.max_orphaned_turns)
            .unwrap_or(8) as usize;
        let flow_executor: Arc<dyn FlowTurnExecutor> = Arc::new(ActorFlowTurnExecutor::new(
            handle.clone(),
            provisioner.clone(),
            max_orphaned_turns,
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
        let spawn_policy = Arc::new(super::spawn_policy::SpawnPolicyService::new());

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
            edge_locks: Arc::new(super::edge_locks::EdgeLockRegistry::new()),
            lifecycle_tasks: tokio::task::JoinSet::new(),
            session_service: handle_session_service,
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter,
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
            pending_supervisor_rotation_fallback,
            spawn_policy,
            dsl_authority: *dsl_authority,
            machine_state_watch_tx,
            phase_watch_tx: phase_watch_tx_actor,
            default_external_tools_provider,
            realm_profile_store,
            composition_binding,
            pending_routed_effects: Vec::new(),
            destroy_cleanup_active: false,
        };
        tokio::spawn(actor.run(command_rx));

        handle
    }
}
