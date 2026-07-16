use super::*;
use crate::machines::mob_machine as mob_dsl;
use crate::store::authority_validating_mob_run_store;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::comms::{
    CommsTrustMutation, CommsTrustMutationAuthority, CommsTrustMutationResult, PeerId, SendError,
    TrustedPeerDescriptor,
};
#[cfg(feature = "runtime-adapter")]
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const MOB_COMMAND_CHANNEL_CAPACITY: usize = 4096;

fn placed_orchestrator_resume_notification_is_non_gating(error: &MobError) -> bool {
    // This turn is informational and runs before the restored controller has
    // re-probed placed hosts. Any well-formed terminal response from the
    // remote host (including a stale-incarnation refusal) is therefore an
    // observation for later reconciliation, not a reason to suppress the
    // controller actor. Local validation/corruption and uncertain trust
    // rollback remain outside this allowlist and fail recovery closed.
    match error {
        MobError::BridgeRequestTimedOut { .. }
        | MobError::BridgeCommandRejected { .. }
        | MobError::BridgeDeliveryRejected { .. } => true,
        MobError::CommsError(error) => matches!(
            error,
            SendError::PeerNotFound(_) | SendError::PeerOffline | SendError::Transport(_)
        ),
        _ => false,
    }
}

fn settle_placed_orchestrator_resume_notification(
    member_id: &AgentIdentity,
    result: Result<(), MobError>,
) -> Result<(), MobError> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if placed_orchestrator_resume_notification_is_non_gating(&error) => {
            tracing::warn!(
                %member_id,
                error = %error,
                "placed orchestrator resume notification was unavailable; continuing recovery"
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Realize the informational turn promised by
/// `notify_orchestrator_on_resume` after the caller has durably resumed the
/// mob and restored its live member material. Placement is machine-owned;
/// local AutonomousHost and TurnDriven realizations retain their existing
/// delivery semantics.
pub(super) async fn realize_orchestrator_resume_notification(
    definition: &MobDefinition,
    orchestrator_entry: &RosterEntry,
    session_service: &dyn MobSessionService,
    provisioner: &dyn MobProvisioner,
    dsl_authority: &crate::machines::mob_machine::MobMachineAuthority,
) -> Result<(), MobError> {
    let dsl_identity = crate::machines::mob_machine::AgentIdentity::from_domain(
        &orchestrator_entry.agent_identity,
    );
    if dsl_authority
        .state()
        .member_restore_failures
        .contains_key(&dsl_identity)
    {
        tracing::warn!(
            member_id = %orchestrator_entry.agent_identity,
            "Skipping orchestrator resume notification because the orchestrator is Broken"
        );
        return Ok(());
    }

    let active_count = dsl_authority
        .state()
        .identity_to_runtime
        .values()
        .filter(|runtime_id| {
            dsl_authority.state().live_runtime_ids.contains(*runtime_id)
                && dsl_authority.state().member_state_markers.get(*runtime_id)
                    != Some(&crate::machines::mob_machine::MobMemberState::Retiring)
        })
        .count();
    let wired_edges = dsl_authority.state().wiring_edges.len();
    let machine_session_id = dsl_authority
        .state()
        .member_session_bindings
        .get(&dsl_identity)
        .ok_or_else(|| {
            MobError::Internal("orchestrator entry missing MobMachine session binding".to_string())
        })?;
    let bridge_session_id =
        meerkat_core::types::SessionId::parse(&machine_session_id.0).map_err(|error| {
            MobError::Internal(format!(
                "orchestrator entry has invalid MobMachine session binding '{}': {error}",
                machine_session_id.0
            ))
        })?;
    let resume_message = format!(
        "Mob '{}' resumed with {} active meerkats and {} wiring links. Reconcile worker state and continue orchestration.",
        definition.id, active_count, wired_edges
    );
    let placed_turn_context = if let Some(host_id) = dsl_authority
        .state()
        .member_placement
        .get(&dsl_identity)
        .cloned()
    {
        let state = dsl_authority.state();
        if state.host_bind_phase.get(&host_id).copied()
            != Some(crate::machines::mob_machine::HostBindPhase::Bound)
        {
            return Err(MobError::Internal(format!(
                "orchestrator resume placement host '{}' is not bound",
                host_id.0
            )));
        }
        let generation = state
            .identity_runtime_generations
            .get(&dsl_identity)
            .copied()
            .ok_or_else(|| {
                MobError::Internal("orchestrator resume missing machine generation".to_string())
            })?;
        let fence_token = state
            .identity_runtime_fence_tokens
            .get(&dsl_identity)
            .copied()
            .ok_or_else(|| {
                MobError::Internal("orchestrator resume missing machine fence token".to_string())
            })?;
        let host_binding_generation = state
            .current_placed_spawn_host_binding_generations
            .get(&dsl_identity)
            .copied()
            .ok_or_else(|| {
                MobError::Internal(
                    "orchestrator resume missing carrier host binding generation".to_string(),
                )
            })?;
        if state.host_binding_generations.get(&host_id).copied() != Some(host_binding_generation) {
            return Err(MobError::Internal(format!(
                "orchestrator resume carrier binding generation {host_binding_generation} is stale against host '{}'",
                host_id.0
            )));
        }
        let crate::event::MemberRef::BackendPeer {
            session_id: route_session_id,
            ..
        } = &orchestrator_entry.member_ref
        else {
            return Err(MobError::Internal(
                "placed orchestrator resume has no remote-session peer route".to_string(),
            ));
        };
        if route_session_id
            .as_ref()
            .is_some_and(|session_id| session_id != &bridge_session_id)
        {
            return Err(MobError::Internal(
                "placed orchestrator resume route is stale against machine session".to_string(),
            ));
        }
        Some(super::provisioner::PlacedTurnDeliveryContext {
            input_id: meerkat_core::time_compat::new_uuid_v7().to_string(),
            transcript_interaction_id: None,
            expected_member: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
                mob_id: definition.id.to_string(),
                agent_identity: orchestrator_entry.agent_identity.to_string(),
                host_id: host_id.0,
                binding_generation: host_binding_generation,
                member_session_id: bridge_session_id.to_string(),
                generation: generation.0,
                fence_token: fence_token.0,
            },
            outcome_tracking: None,
        })
    } else {
        None
    };
    let turn_member_ref = match (&orchestrator_entry.member_ref, &placed_turn_context) {
        (
            crate::event::MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                ..
            },
            Some(_),
        ) => crate::event::MemberRef::BackendPeer {
            peer_id: peer_id.clone(),
            address: address.clone(),
            pubkey: *pubkey,
            bootstrap_token: bootstrap_token.clone(),
            session_id: Some(bridge_session_id.clone()),
        },
        _ => orchestrator_entry.member_ref.clone(),
    };
    if placed_turn_context.is_some() {
        // Placement selects the owner before runtime mode. A placed
        // AutonomousHost session belongs to the member host just like a
        // placed TurnDriven one; never use this controller's injector.
        let notification = provisioner
            .start_turn_with_correlation(
                &turn_member_ref,
                meerkat_core::service::StartTurnRequest {
                    injected_context: Vec::new(),
                    prompt: resume_message.into(),
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
                },
                placed_turn_context,
            )
            .await
            .map(|_| ());
        settle_placed_orchestrator_resume_notification(
            &orchestrator_entry.agent_identity,
            notification,
        )?;
    } else {
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
                    .start_turn_with_correlation(
                        &turn_member_ref,
                        meerkat_core::service::StartTurnRequest {
                            injected_context: Vec::new(),
                            prompt: resume_message.into(),
                            system_prompt: None,
                            event_tx: None,
                            runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
                        },
                        None,
                    )
                    .await?;
            }
        }
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
async fn drive_recovered_placed_lifecycle_intent(
    handle: MobHandle,
    intent: mob_dsl::PlacedCompletionLifecycleIntentKind,
) {
    let mut retry_delay = std::time::Duration::from_millis(25);
    loop {
        if handle.command_tx.is_closed() {
            return;
        }
        let result = match intent {
            mob_dsl::PlacedCompletionLifecycleIntentKind::Stop => handle.stop().await,
            mob_dsl::PlacedCompletionLifecycleIntentKind::Reset => handle.reset().await,
            mob_dsl::PlacedCompletionLifecycleIntentKind::Complete => handle.complete().await,
            mob_dsl::PlacedCompletionLifecycleIntentKind::RetireAll => handle.retire_all().await,
            mob_dsl::PlacedCompletionLifecycleIntentKind::Destroy => handle
                .destroy()
                .await
                .map(|_| ())
                .map_err(|error| MobError::Internal(error.to_string())),
        };
        match result {
            Ok(()) => return,
            Err(MobError::ActorCommandChannelClosed) => return,
            Err(error) => {
                tracing::warn!(
                    mob_id = %handle.mob_id(),
                    ?intent,
                    error = %error,
                    retry_delay_ms = retry_delay.as_millis(),
                    "automatic recovery of typed placed lifecycle intent remains incomplete"
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(std::time::Duration::from_secs(2));
            }
        }
    }
}

#[cfg(test)]
static NEXT_TEST_ACTOR_RUNTIME_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);
#[cfg(test)]
static LATEST_TEST_ACTOR_RUNTIME_BY_MOB: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<String, u64>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));
#[cfg(test)]
static ACTOR_OWNED_STARTUP_WORKERS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<u64, usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));

#[cfg(test)]
struct ActorOwnedStartupWorkerGuard {
    actor_runtime_id: u64,
}

#[cfg(test)]
impl ActorOwnedStartupWorkerGuard {
    fn new(actor_runtime_id: u64) -> Self {
        let mut workers = ACTOR_OWNED_STARTUP_WORKERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *workers.entry(actor_runtime_id).or_default() += 1;
        Self { actor_runtime_id }
    }
}

#[cfg(test)]
impl Drop for ActorOwnedStartupWorkerGuard {
    fn drop(&mut self) {
        let mut workers = ACTOR_OWNED_STARTUP_WORKERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let std::collections::btree_map::Entry::Occupied(mut entry) =
            workers.entry(self.actor_runtime_id)
        {
            if *entry.get() > 1 {
                *entry.get_mut() -= 1;
            } else {
                entry.remove();
            }
        }
    }
}

#[cfg(test)]
fn register_actor_runtime_for_test(mob_id: &MobId) -> u64 {
    let actor_runtime_id =
        NEXT_TEST_ACTOR_RUNTIME_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    LATEST_TEST_ACTOR_RUNTIME_BY_MOB
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(mob_id.to_string(), actor_runtime_id);
    actor_runtime_id
}

#[cfg(test)]
pub(super) fn latest_actor_owned_startup_worker_count_for_test(mob_id: &MobId) -> usize {
    let actor_runtime_id = LATEST_TEST_ACTOR_RUNTIME_BY_MOB
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(mob_id.as_str())
        .copied();
    let Some(actor_runtime_id) = actor_runtime_id else {
        return 0;
    };
    ACTOR_OWNED_STARTUP_WORKERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&actor_runtime_id)
        .copied()
        .unwrap_or(0)
}

struct ResumeDesiredTrust {
    spec: TrustedPeerDescriptor,
    source: ResumeTrustSource,
}

struct ResumeTrustMutation {
    comms: Arc<dyn CoreCommsRuntime>,
    operation: ResumeTrustMutationOperation,
    authority: CommsTrustMutationAuthority,
}

struct ResumePeerOnlyTrustReconcile {
    member_ref: MemberRef,
    agent_identity: AgentIdentity,
    desired_peer_trust: super::provisioner::PeerOnlyTrustOverlay,
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
    spawn_base_prompt_source: Option<Arc<dyn super::spec_compiler::SpawnBasePromptSource>>,
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
    /// Controlling-side reverse-lane acceptor composition (ADJ-P4-2).
    /// `None` means mixed-host routes that need a local reverse lane fail
    /// closed instead of publishing the member's process-local endpoint.
    controlling_acceptor: Option<ControllingAcceptorConfig>,
    /// Phase 6b (ADJ-P6B-1/-16): the LOCAL-branch live gateway for the
    /// identity-addressed `member_live_*` verbs — the session-id-addressed
    /// entry to THE ONE extracted live pipeline
    /// (`meerkat_runtime::member_live::MemberLiveHost`). Production wiring:
    /// live-capable surfaces (rkat-rpc's live composition; `rkat mob host`
    /// for its own mobs) pass their `ServiceMemberLiveHost` through this
    /// knob. `None` (the default) = local members' live verbs typed-reject
    /// `LiveTransportUnavailable` — honest degradation, zero cost.
    member_live_host: Option<Arc<dyn meerkat_runtime::member_live::MemberLiveHost>>,
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

/// Reverse-lane registration material for one local session-backed member
/// (multi-host §10.4, ADJ-P4-2): the Ed25519 signing keypair + inbox sender
/// the controlling-side `HostAcceptor` demux needs to serve inbound
/// envelopes addressed to the member from a remote member host (acks are
/// signed by the ADDRESSED identity's keypair).
pub struct MemberAcceptorRegistration {
    pub pubkey: meerkat_comms::PubKey,
    pub keypair: Arc<meerkat_comms::Keypair>,
    pub inbox_sender: meerkat_comms::InboxSender,
}

/// Source of controlling-side acceptor registration material for local
/// members. The COMPOSING host implements this where it can reach the
/// concrete member comms runtime (the factory/session substrate); the mob
/// runtime itself only ever holds the trait-erased `CoreCommsRuntime`,
/// whose TYPED surface exposes no signing material — the default source
/// decodes the runtime's opaque
/// `host_acceptor_registration_payload()` instead. Returning `None` for a
/// session means that member's cross-host descriptor keeps its
/// machine-recorded canonical endpoint (in-process routing still works via
/// the process-global inproc registry; a real remote process fails closed
/// at the transport until material is supplied).
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait LocalMemberAcceptorMaterialSource: Send + Sync {
    async fn registration_for(&self, session_id: &SessionId) -> Option<MemberAcceptorRegistration>;
}

/// Default [`LocalMemberAcceptorMaterialSource`]: resolve the member
/// session's live comms runtime through the mob session service and decode
/// the concrete runtime's opaque acceptor-registration payload
/// ([`meerkat_comms::HostAcceptorRegistrationMaterial`]). A session without
/// a live runtime, or a runtime that exposes no payload, yields `None` —
/// fail closed, the reverse lane simply stays uncomposed for that member.
#[cfg(not(target_arch = "wasm32"))]
struct SessionServiceAcceptorMaterialSource {
    service: Arc<dyn MobSessionService>,
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl LocalMemberAcceptorMaterialSource for SessionServiceAcceptorMaterialSource {
    async fn registration_for(&self, session_id: &SessionId) -> Option<MemberAcceptorRegistration> {
        let runtime = self.service.comms_runtime(session_id).await?;
        let payload = runtime.host_acceptor_registration_payload()?;
        let material = payload
            .downcast::<meerkat_comms::HostAcceptorRegistrationMaterial>()
            .ok()?;
        Some(MemberAcceptorRegistration {
            pubkey: material.pubkey,
            keypair: Arc::clone(&material.keypair),
            inbox_sender: material.inbox_sender.clone(),
        })
    }
}

/// Process-scoped controlling-side reverse-lane acceptor. One instance is
/// shared by every mob runtime hosted in the process (D1); actors hold only a
/// registration capability and never create their own listeners.
#[cfg(not(target_arch = "wasm32"))]
struct SharedControllingAcceptor {
    listen_tcp: std::net::SocketAddr,
    advertise_address: Option<String>,
    bounds: meerkat_comms::HostAcceptorBounds,
    live: tokio::sync::Mutex<Option<SharedControllingAcceptorLive>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct SharedControllingAcceptorLive {
    registry: Arc<meerkat_comms::HostAcceptorIdentityRegistry>,
    owner: Arc<dyn std::any::Any + Send + Sync>,
    advertised_address: String,
    leases: BTreeMap<String, SharedControllingAcceptorRegistration>,
    _handle: meerkat_comms::HostAcceptorHandle,
}

#[cfg(not(target_arch = "wasm32"))]
struct SharedControllingAcceptorRegistration {
    logical_owner: String,
    token: Arc<()>,
}

/// Exact registration incarnation. Compare-remove prevents an old mob actor
/// from deleting the replacement inbox for the same durable signing key.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub(super) struct ControllingAcceptorRegistrationLease {
    pubkey: meerkat_comms::PubKey,
    pub(super) advertised_address: String,
    key: String,
    pub(super) token: Arc<()>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SharedControllingAcceptor {
    fn new(
        listen_tcp: std::net::SocketAddr,
        advertise_address: Option<String>,
        bounds: meerkat_comms::HostAcceptorBounds,
    ) -> Self {
        Self {
            listen_tcp,
            advertise_address,
            bounds,
            live: tokio::sync::Mutex::new(None),
        }
    }

    async fn register(
        &self,
        logical_owner: String,
        registration: MemberAcceptorRegistration,
    ) -> Result<ControllingAcceptorRegistrationLease, MobError> {
        let mut live = self.live.lock().await;
        if live.is_none() {
            let registry = Arc::new(meerkat_comms::HostAcceptorIdentityRegistry::new());
            let owner: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
            registry
                .install_owner(Arc::clone(&owner))
                .map_err(|error| {
                    MobError::WiringError(format!(
                        "controlling acceptor owner install failed: {error}"
                    ))
                })?;
            let handle = meerkat_comms::spawn_host_acceptor(meerkat_comms::HostAcceptorConfig {
                listen_tcp: self.listen_tcp,
                advertise_address: self.advertise_address.clone(),
                registry: Arc::clone(&registry),
                pairing: None,
                bounds: self.bounds,
            })
            .await
            .map_err(|error| {
                MobError::WiringError(format!("controlling acceptor bind failed: {error}"))
            })?;
            let advertised_address = handle.advertised_address().to_string();
            *live = Some(SharedControllingAcceptorLive {
                registry,
                owner,
                advertised_address,
                leases: BTreeMap::new(),
                _handle: handle,
            });
        }
        let live = live
            .as_mut()
            .expect("controlling acceptor initialized above");
        let key = registration.pubkey.to_pubkey_string();
        if let Some(existing) = live.leases.get(&key)
            && existing.logical_owner != logical_owner
        {
            return Err(MobError::WiringError(format!(
                "controlling acceptor identity '{key}' is already owned by a different mob member"
            )));
        }
        match live.registry.register_identity(
            &live.owner,
            registration.pubkey,
            Arc::clone(&registration.keypair),
            registration.inbox_sender.clone(),
        ) {
            Ok(()) => {}
            Err(meerkat_comms::HostAcceptorError::IdentityAlreadyRegistered { .. }) => {
                // A durable session key can survive an actor replacement while
                // its inbox cannot. Refresh the stale projection under the
                // process acceptor capability; a lookup in the narrow
                // remove/register handoff may reject and be retried.
                live.registry
                    .remove_identity(&live.owner, &registration.pubkey)
                    .and_then(|_| {
                        live.registry.register_identity(
                            &live.owner,
                            registration.pubkey,
                            registration.keypair,
                            registration.inbox_sender,
                        )
                    })
                    .map_err(|error| {
                        MobError::WiringError(format!(
                            "controlling acceptor identity replacement failed: {error}"
                        ))
                    })?;
            }
            Err(error) => {
                return Err(MobError::WiringError(format!(
                    "controlling acceptor identity registration failed: {error}"
                )));
            }
        }
        let token = Arc::new(());
        live.leases.insert(
            key.clone(),
            SharedControllingAcceptorRegistration {
                logical_owner,
                token: Arc::clone(&token),
            },
        );
        Ok(ControllingAcceptorRegistrationLease {
            pubkey: registration.pubkey,
            advertised_address: live.advertised_address.clone(),
            key,
            token,
        })
    }

    async fn remove(&self, lease: &ControllingAcceptorRegistrationLease) -> Result<(), MobError> {
        let mut live = self.live.lock().await;
        let Some(live) = live.as_mut() else {
            return Ok(());
        };
        let Some(current) = live.leases.get(&lease.key) else {
            return Ok(());
        };
        if !Arc::ptr_eq(&current.token, &lease.token) {
            return Ok(());
        }
        live.registry
            .remove_identity(&live.owner, &lease.pubkey)
            .map_err(|error| {
                MobError::WiringError(format!(
                    "controlling acceptor identity removal failed: {error}"
                ))
            })?;
        live.leases.remove(&lease.key);
        Ok(())
    }
}

/// Controlling-side reverse-lane composition input (ADJ-P4-2): a shared
/// host-process listener plus the local session material source. Clone this
/// value into every [`MobBuilder`] in one process so all mobs use one ingress.
#[derive(Clone)]
pub struct ControllingAcceptorConfig {
    /// Registration material source for local members.
    pub material: Arc<dyn LocalMemberAcceptorMaterialSource>,
    #[cfg(not(target_arch = "wasm32"))]
    shared: Arc<SharedControllingAcceptor>,
}

impl ControllingAcceptorConfig {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(
        listen_tcp: std::net::SocketAddr,
        advertise_address: Option<String>,
        material: Arc<dyn LocalMemberAcceptorMaterialSource>,
    ) -> Self {
        Self {
            material,
            shared: Arc::new(SharedControllingAcceptor::new(
                listen_tcp,
                advertise_address,
                meerkat_comms::HostAcceptorBounds::default(),
            )),
        }
    }

    /// Build the ordinary process acceptor from the same session substrate
    /// that owns local mob member runtimes.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn for_session_service(
        listen_tcp: std::net::SocketAddr,
        advertise_address: Option<String>,
        service: Arc<dyn MobSessionService>,
    ) -> Self {
        Self::new(
            listen_tcp,
            advertise_address,
            Arc::new(SessionServiceAcceptorMaterialSource { service }),
        )
    }

    /// Resolve the process host-ingress composition from effective config.
    /// The same `[mob_host]` listener facts serve both a dedicated member-host
    /// daemon and a controlling process that owns local mob members.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_mob_host_config(
        config: &meerkat_core::config::MobHostConfig,
        service: Arc<dyn MobSessionService>,
    ) -> Result<Option<Self>, String> {
        let Some(listen) = config.listen_tcp.as_deref() else {
            if config.advertise_tcp.is_some() {
                return Err(
                    "[mob_host].advertise_tcp requires [mob_host].listen_tcp for host-process ingress"
                        .to_string(),
                );
            }
            return Ok(None);
        };
        let listen_tcp = listen
            .parse::<std::net::SocketAddr>()
            .map_err(|error| format!("invalid [mob_host].listen_tcp '{listen}': {error}"))?;
        let mut bounds = meerkat_comms::HostAcceptorBounds::default();
        if let Some(max_connections) = config.max_connections {
            if max_connections == 0 {
                return Err("[mob_host].max_connections must be greater than zero".to_string());
            }
            bounds.max_connections = max_connections;
        }
        if let Some(read_deadline_ms) = config.read_deadline_ms {
            if read_deadline_ms == 0 {
                return Err("[mob_host].read_deadline_ms must be greater than zero".to_string());
            }
            bounds.read_deadline = std::time::Duration::from_millis(read_deadline_ms);
        }
        if let Some(pairing_rate) = config.pairing_rate {
            if pairing_rate == 0 {
                return Err("[mob_host].pairing_rate must be greater than zero".to_string());
            }
            bounds.pairing_rate = meerkat_comms::PairingRateLimit {
                max_attempts: pairing_rate,
                window: std::time::Duration::from_secs(60),
            };
        }
        Ok(Some(Self {
            material: Arc::new(SessionServiceAcceptorMaterialSource { service }),
            shared: Arc::new(SharedControllingAcceptor::new(
                listen_tcp,
                config.advertise_tcp.clone(),
                bounds,
            )),
        }))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) async fn register(
        &self,
        logical_owner: String,
        registration: MemberAcceptorRegistration,
    ) -> Result<ControllingAcceptorRegistrationLease, MobError> {
        self.shared.register(logical_owner, registration).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) async fn remove(
        &self,
        lease: &ControllingAcceptorRegistrationLease,
    ) -> Result<(), MobError> {
        self.shared.remove(lease).await
    }
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
    agent_runtime_id: &crate::ids::AgentRuntimeId,
    generation: crate::ids::Generation,
    fence_token: crate::ids::FenceToken,
    peer_descriptor: &meerkat_core::comms::TrustedPeerDescriptor,
    context: &'static str,
) -> Result<(), MobError> {
    let dsl_identity = crate::machines::mob_machine::AgentIdentity::from_domain(agent_identity);
    let candidate = crate::machines::mob_machine::MemberPeerEndpoint::from(peer_descriptor);
    if let Some(existing) = authority.state().member_peer_endpoints.get(&dsl_identity) {
        if existing != &candidate {
            return Err(MobError::WiringError(format!(
                "{context}: live endpoint for '{agent_identity}' disagrees with its durable generation binding"
            )));
        }
    }
    // Always enter generated authority, even for an idempotent endpoint. The
    // exact runtime/generation/fence tuple must still be admitted; matching an
    // identity-keyed descriptor alone cannot validate incarnation authority.
    apply_seeded_mob_input(
        authority,
        crate::machines::mob_machine::MobMachineInput::RegisterMemberPeer {
            agent_identity: dsl_identity,
            agent_runtime_id: crate::machines::mob_machine::AgentRuntimeId::from_domain(
                agent_runtime_id,
            ),
            generation: crate::machines::mob_machine::Generation::from_domain(generation),
            fence_token: crate::machines::mob_machine::FenceToken::from_domain(fence_token),
            peer_endpoint: candidate,
        },
        context,
    )
}

fn recovered_session_bindings_without_endpoint(
    events: &[crate::event::MobEvent],
) -> HashMap<AgentIdentity, (AgentRuntimeId, SessionId)> {
    let mut recovered = HashMap::new();
    for event in events {
        match &event.kind {
            MobEventKind::MemberSpawned(member_spawned) => {
                recovered.remove(&member_spawned.agent_identity);
            }
            MobEventKind::MemberSessionBindingRecovered(binding) => {
                if let Some(bridge_session_id) = binding.bridge_session_id() {
                    if binding.member_peer_endpoint().is_none() {
                        recovered.insert(
                            binding.agent_identity.clone(),
                            (binding.agent_runtime_id.clone(), bridge_session_id.clone()),
                        );
                    } else {
                        recovered.remove(&binding.agent_identity);
                    }
                }
            }
            MobEventKind::MemberReset { agent_identity, .. }
            | MobEventKind::MemberRetired { agent_identity, .. } => {
                recovered.remove(agent_identity);
            }
            _ => {}
        }
    }
    recovered
}

pub(super) async fn append_recovered_session_binding(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    events: &Arc<dyn MobEventStore>,
    mob_id: &MobId,
    entry: &RosterEntry,
    bridge_session_id: &SessionId,
    member_peer_endpoint: Option<TrustedPeerDescriptor>,
    context: &'static str,
) -> Result<crate::event::MobEvent, MobError> {
    let dsl_identity =
        crate::machines::mob_machine::AgentIdentity::from_domain(&entry.agent_identity);
    let dsl_runtime_id =
        crate::machines::mob_machine::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
    let dsl_session_id = crate::machines::mob_machine::SessionId::from_domain(bridge_session_id);
    if authority.state().identity_to_runtime.get(&dsl_identity) != Some(&dsl_runtime_id) {
        return Err(MobError::WiringError(format!(
            "{context}: recovered binding for '{}' does not match the current runtime incarnation",
            entry.agent_identity
        )));
    }

    // One write-ahead transaction: stage BOTH the replacement session binding
    // and its optional exact endpoint on a detached authority. Endpoint drift
    // or event-store failure must leave the actor's live machine projection on
    // the old binding. The durable public event is the replay authority, so
    // publish the staged machine only after append succeeds.
    let mut prepared = authority.prepare_authority();
    let replacing = prepared
        .state()
        .member_session_bindings
        .get(&dsl_identity)
        .cloned();
    prepared
        .apply_signal(
            crate::machines::mob_machine::MobMachineSignal::RecoverMemberSessionBinding {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: dsl_runtime_id.clone(),
                bridge_session_id: dsl_session_id.clone(),
                replacing,
            },
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "MobMachine prepared authority ({context}) rejected recovered binding for '{}': {error}",
                entry.agent_identity
            ))
        })?;
    if let Some(endpoint) = member_peer_endpoint.as_ref() {
        prepared
            .apply_signal(
                crate::machines::mob_machine::MobMachineSignal::RecoverMemberPeerEndpoint {
                agent_identity: dsl_identity,
                agent_runtime_id: dsl_runtime_id,
                bridge_session_id: dsl_session_id,
                peer_endpoint: crate::machines::mob_machine::MemberPeerEndpoint::from(endpoint),
                },
            )
            .map_err(|error| {
                MobError::Internal(format!(
                    "MobMachine prepared authority ({context}) rejected recovered endpoint for '{}': {error}",
                    entry.agent_identity
                ))
            })?;
    }

    let event = events
        .append(crate::event::NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: MobEventKind::MemberSessionBindingRecovered(
                crate::event::MemberSessionBindingRecoveredEvent::new(
                    entry.agent_identity.clone(),
                    entry.agent_runtime_id.clone(),
                    bridge_session_id.clone(),
                )
                .with_member_peer_endpoint(member_peer_endpoint),
            ),
        })
        .await?;
    authority
        .commit_prepared_authority(prepared)
        .map_err(|error| {
            MobError::Internal(format!(
                "{context}: recovered binding authority changed before commit for '{}': {error}",
                entry.agent_identity
            ))
        })?;
    Ok(event)
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
    for (index, mutation) in mutations.iter().enumerate() {
        if mutation.authority.is_mob_machine_source() {
            mutation
                .comms
                .validate_recovered_generated_mob_trust_owner(Arc::clone(mob_owner_token))
                .await?;
        }
        match &mutation.operation {
            ResumeTrustMutationOperation::Add(peer) => {
                for prior in &mutations[..index] {
                    let ResumeTrustMutationOperation::Add(prior_peer) = &prior.operation else {
                        continue;
                    };
                    if Arc::ptr_eq(&mutation.comms, &prior.comms)
                        && prior_peer.peer_id == peer.peer_id
                        && prior_peer != peer
                    {
                        return Err(SendError::Validation(format!(
                            "resume trust repair carries conflicting descriptors for peer {} on the same runtime",
                            peer.peer_id
                        )));
                    }
                }
                mutation
                    .authority
                    .preflight_public_add(mutation.comms.peer_id(), peer)
                    .map_err(SendError::Validation)?;
                TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                    .map_err(SendError::Validation)?;
                if let Some(existing) = mutation
                    .comms
                    .peers()
                    .await
                    .into_iter()
                    .find(|existing| existing.peer_id == peer.peer_id)
                    && (existing.name != peer.name || existing.address != peer.address)
                {
                    return Err(SendError::Validation(format!(
                        "resume trust repair for peer {} conflicts with aggregate live descriptor name='{}' address='{}'",
                        peer.peer_id, existing.name, existing.address
                    )));
                }
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

fn resume_member_endpoint_migration_cleanup_authority(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    topology_epoch: &Arc<std::sync::atomic::AtomicU64>,
    edge: &crate::machines::mob_machine::WiringEdge,
    migrated_identity: &crate::ids::AgentIdentity,
    migrated_runtime_id: &AgentRuntimeId,
    retained_peer_endpoint: &crate::machines::mob_machine::MemberPeerEndpoint,
    context: &'static str,
) -> Result<CommsTrustMutationAuthority, MobError> {
    let dsl_identity = crate::machines::mob_machine::AgentIdentity::from_domain(migrated_identity);
    let transition = apply_seeded_mob_input_collect_transition(
        authority,
        crate::machines::mob_machine::MobMachineInput::AuthorizeMemberEndpointMigrationTrustCleanup {
            edge: edge.clone(),
            agent_identity: dsl_identity,
            agent_runtime_id:
                crate::machines::mob_machine::AgentRuntimeId::from_domain(migrated_runtime_id),
            retained_peer_endpoint: retained_peer_endpoint.clone(),
        },
        context,
    )?;
    let retained_peer_id = retained_peer_endpoint.peer_id.0.as_str();
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
            migrated_identity.as_str(),
            retained_peer_id,
        )
        .ok()
    })
    .ok_or_else(|| {
        MobError::WiringError(format!(
            "{context} produced no generated endpoint-migration cleanup authority for historical peer '{retained_peer_id}'"
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

/// Reconcile machine-owned trust topology after resume materializes the live
/// member incarnations. This is the single resume seam for peer-only rebind,
/// local trust mutation, and peer-only trust projection.
#[cfg(feature = "runtime-adapter")]
#[allow(clippy::too_many_arguments)]
pub(super) async fn reconcile_resume_topology(
    definition: &Arc<MobDefinition>,
    roster: &mut Roster,
    provisioner: &dyn MobProvisioner,
    supervisor_bridge: &Arc<MobSupervisorBridge>,
    runtime_metadata: &Arc<dyn crate::store::MobRuntimeMetadataStore>,
    dsl_authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    topology_epoch: &Arc<std::sync::atomic::AtomicU64>,
) -> Result<(), MobError> {
    // A pending exact supervisor operation owns its target peers until its
    // observation/retry path completes; ordinary resume must not race it.
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
    let mut entries = roster.list().cloned().collect::<Vec<_>>();
    let machine_wiring_edges = dsl_authority.state().wiring_edges.clone();
    let machine_external_peer_edges = dsl_authority.state().external_peer_edges.clone();
    let retiring_identities = dsl_authority
        .state()
        .identity_to_runtime
        .iter()
        .filter_map(|(identity, runtime_id)| {
            (dsl_authority.state().member_state_markers.get(runtime_id)
                == Some(&crate::machines::mob_machine::MobMemberState::Retiring))
            .then_some(identity.clone())
        })
        .collect::<HashSet<_>>();
    let broken_members = dsl_authority
        .state()
        .member_restore_failures
        .keys()
        .map(|identity| AgentIdentity::from(identity.0.as_str()))
        .collect::<HashSet<_>>();
    let mob_owner_token = dsl_authority.generated_authority_owner_token();

    // Legacy MemberSpawned journals predate the replay-only endpoint field.
    // Recover a peer-only member's exact endpoint from its durable MemberRef
    // before supervisor rebind classification: that generated classifier
    // requires MobMachine endpoint authority and must not depend on the later
    // trust-projection pass to manufacture it.
    for entry in &entries {
        if broken_members.contains(&entry.agent_identity)
            || recovered_endpoint_runtime_is_retiring(
                dsl_authority.state(),
                &crate::machines::mob_machine::AgentIdentity::from_domain(&entry.agent_identity),
            )
            || super::member_runtime_is_host_owned(dsl_authority.state(), &entry.agent_identity)
        {
            continue;
        }
        let MemberRef::BackendPeer {
            peer_id,
            session_id: None,
            ..
        } = &entry.member_ref
        else {
            continue;
        };
        let dsl_identity =
            crate::machines::mob_machine::AgentIdentity::from_domain(&entry.agent_identity);
        if dsl_authority
            .state()
            .member_peer_endpoints
            .contains_key(&dsl_identity)
        {
            continue;
        }
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
            &entry.agent_runtime_id,
            entry.generation,
            entry.fence_token,
            &spec,
            "resume_register_legacy_backend_member_peer_before_rebind",
        )?;
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
        if broken_members.contains(&entry.agent_identity) {
            continue;
        }
        // Placement must be classified before the generic composite
        // provisioner sees a BackendPeer(Some(remote_session_id)).
        if super::member_runtime_is_host_owned(dsl_authority.state(), &entry.agent_identity) {
            continue;
        }
        if provisioner.comms_runtime(&entry.member_ref).await.is_some() {
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
                rebind_observation.rejection_cause.clone(),
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
            let (updated_member_ref, rebind_authority) = apply_resume_peer_only_rebind_authority(
                dsl_authority,
                roster,
                runtime_metadata,
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
    let mut resume_peer_only_trust_reconciliations = Vec::new();
    for entry in &entries {
        if broken_members.contains(&entry.agent_identity) {
            continue;
        }
        let local_dsl_identity =
            crate::machines::mob_machine::AgentIdentity::from_domain(&entry.agent_identity);
        let placed =
            super::member_runtime_is_host_owned(dsl_authority.state(), &entry.agent_identity);
        let local_comms = if placed {
            None
        } else {
            provisioner.comms_runtime(&entry.member_ref).await
        };
        let local_peer_id = local_comms.as_ref().and_then(|comms| comms.peer_id());
        let mut desired_trust = Vec::new();

        for edge in &machine_wiring_edges {
            let peer_dsl_identity = if edge.a == local_dsl_identity {
                &edge.b
            } else if edge.b == local_dsl_identity {
                &edge.a
            } else {
                continue;
            };
            if !super::recovery_member_edge_trust_is_desired(dsl_authority.state(), edge) {
                // A durable Started carrier owns this edge's recovery
                // intent. Retirement will remove old trust; resume must
                // never repair/reinstall it in the meantime.
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
            let retained_peer_endpoints = dsl_authority
                .state()
                .member_prior_peer_endpoints
                .get(peer_dsl_identity)
                .cloned()
                .unwrap_or_default();
            let retained_peer_ids = retained_peer_endpoints
                .iter()
                .map(|endpoint| endpoint.peer_id.0.clone())
                .collect::<Vec<_>>();
            if let Some(comms_a) = local_comms.as_ref() {
                for retained_peer_endpoint in &retained_peer_endpoints {
                    let retained_peer_id = retained_peer_endpoint.peer_id.0.as_str();
                    let cleanup_authority = resume_member_endpoint_migration_cleanup_authority(
                        dsl_authority,
                        topology_epoch,
                        edge,
                        &peer_entry.agent_identity,
                        &peer_entry.agent_runtime_id,
                        retained_peer_endpoint,
                        "resume_member_endpoint_migration_cleanup",
                    )?;
                    resume_trust_mutations.push(ResumeTrustMutation {
                        comms: Arc::clone(comms_a),
                        operation: ResumeTrustMutationOperation::Remove(
                            retained_peer_id.to_string(),
                        ),
                        authority: cleanup_authority,
                    });
                }
            }
            // A durable retirement-start marker blocks ordinary trust
            // repair, but not exact historical cleanup. Sweep retained
            // generation endpoints first; recreating either current side
            // would undo scoped retirement cleanup, so current repair still
            // remains deferred to the actor's idempotent retirement retry.
            if !recovered_member_edge_allows_trust_repair(dsl_authority.state(), edge) {
                continue;
            }
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
                        if retained_peer_ids
                            .iter()
                            .any(|peer_id| peer_id == &stale_peer.peer_id.to_string())
                        {
                            // The exact historical row is already covered
                            // by the generated migration cleanup above.
                            // Never reinterpret it as the broken member's
                            // current endpoint.
                            continue;
                        }
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
                                &peer_entry.agent_runtime_id,
                                peer_entry.generation,
                                peer_entry.fence_token,
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
                && !retiring_identities.contains(&local_dsl_identity)
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
            // §19.L5/DEC-R1: placed members are not peer-only externals —
            // their wiring trust installs are the phase-4 cross-host
            // lane, never a resume-time dial of the member endpoint.
            if placed {
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
            // This bridge call replaces remote trust state. Defer it until
            // every local add/remove authority has passed preflight so a
            // later deterministic local rejection cannot leave a mixed
            // topology with only the peer-only side updated.
            resume_peer_only_trust_reconciliations.push(ResumePeerOnlyTrustReconcile {
                member_ref: entry.member_ref.clone(),
                agent_identity: entry.agent_identity.clone(),
                desired_peer_trust,
            });
            continue;
        };
        for desired in &desired_trust {
            let spec_peer_id = desired.spec.peer_id.to_string();
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
    for reconcile in resume_peer_only_trust_reconciliations {
        let report = provisioner
            .reconcile_peer_only_trust(
                &reconcile.member_ref,
                Some(&reconcile.desired_peer_trust),
                None,
            )
            .await?;
        if let Some(rebind_observation) = report.rebind_required {
            // The rebind prepass already reconciled supervisor authority
            // for this member, so a rejection here is bubbled up with its
            // raw cause; MobMachine owns recoverable-vs-fatal
            // classification and it is not re-derived here.
            return Err(MobError::BridgeCommandRejected {
                cause: rebind_observation.rejection_cause,
                reason: format!(
                    "resume peer-only trust reconcile for '{}' was rejected after MobMachine rebind prepass",
                    reconcile.agent_identity
                ),
            });
        }
    }
    Ok(())
}

pub(super) fn apply_seeded_member_session_binding(
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

pub(super) async fn latest_persisted_session_for_member(
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
    if let Some(binding) = metadata.mob_member_binding.as_ref() {
        // Typed durable ownership is authoritative. The comms-name fallback
        // exists only for journals written before MobMemberBinding; it must
        // never override a present but contradictory typed identity.
        return binding.mob_id == mob_id.as_str()
            && binding.role == role.as_str()
            && binding.member == agent_identity.as_str();
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

    if matches!(
        target_phase,
        MobState::Stopped | MobState::Completed | MobState::Destroyed
    ) {
        let desired_intent = match target_phase {
            MobState::Stopped => mob_dsl::PlacedCompletionLifecycleIntentKind::Stop,
            MobState::Completed => mob_dsl::PlacedCompletionLifecycleIntentKind::Complete,
            MobState::Destroyed => mob_dsl::PlacedCompletionLifecycleIntentKind::Destroy,
            MobState::Creating | MobState::Running => return Ok(()),
        };
        if authority.state().placed_completion_lifecycle_intent != Some(desired_intent) {
            apply_seeded_mob_input(
                authority,
                mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
                    intent: desired_intent,
                },
                "seed_phase_completion_quiesce",
            )?;
        }
    }
    match target_phase {
        MobState::Creating | MobState::Running => Ok(()),
        MobState::Stopped => {
            apply_seeded_mob_input(authority, mob_dsl::MobMachineInput::Stop, "seed_phase_stop")
        }
        MobState::Completed => {
            let state = authority.state();
            let cleanup_custody = !state.pending_remote_turn_outcomes.is_empty()
                || !state.committed_remote_turn_outcomes.is_empty()
                || !state.resolved_remote_turn_outcomes.is_empty()
                || !state.pending_placed_completion_outcomes.is_empty()
                || !state.resolved_placed_completion_outcomes.is_empty()
                || !state.pending_placed_kickoff_outcomes.is_empty()
                || !state.resolved_placed_kickoff_outcomes.is_empty();
            if cleanup_custody {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverCompletedWithCleanupCustody,
                    "seed_phase_completed_with_cleanup_custody",
                )
            } else {
                apply_seeded_mob_input(
                    authority,
                    mob_dsl::MobMachineInput::Complete,
                    "seed_phase_complete",
                )
            }
        }
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
/// ADJ-8: seed value for the actor's mechanical fence counter — one above
/// the maximum fence token recorded anywhere in the recovered machine state
/// (identity-keyed and runtime-id-keyed maps both scanned; an empty mob
/// seeds 1 exactly like a fresh build).
fn next_fence_token_seed(state: &crate::machines::mob_machine::MobMachineState) -> u64 {
    let identity_max = state
        .identity_runtime_fence_tokens
        .values()
        .map(|fence| fence.0)
        .max()
        .unwrap_or(0);
    let runtime_max = state
        .runtime_fence_tokens
        .values()
        .map(|fence| fence.0)
        .max()
        .unwrap_or(0);
    identity_max.max(runtime_max).checked_add(1).unwrap_or(0)
}

/// Merge independently recovered fence allocators. Zero is the exhausted
/// runtime sentinel, so it dominates every still-allocatable seed; plain
/// `max` would accidentally reopen allocation when either durable source has
/// already consumed `u64::MAX`.
fn combine_recovered_fence_token_seeds(machine_seed: u64, carrier_seed: u64) -> u64 {
    if machine_seed == 0 || carrier_seed == 0 {
        0
    } else {
        machine_seed.max(carrier_seed)
    }
}

/// T3 route-install recovery (multi-host §10.4, ADJ-P4-1): after placement
/// and host-bind facts are recovered, RECORD the Install obligations derived
/// from durable `wiring_edges` × `member_placement` for hosts recovered
/// `Bound`. `pending_route_installs` is volatile by design (the
/// `pending_recipient_trust` posture) — every fact needed to REBUILD it is
/// durable, and over-recording is safe end-to-end (`RecordRouteInstall` is a
/// set insert, `InstallPeerTrust` a trust upsert, `ResolveRouteInstall` an
/// idempotent removal). Machine guard rejects (e.g. a host that lost its
/// bind between fact recovery and record) are debug-log skips: the T2 drain
/// re-derives them when the host binds. The emitted `RouteInstallRequested`
/// effects are deliberately dropped here — recording rebuilds the ledger;
/// realization is event-driven off the recovery path.
#[cfg(feature = "runtime-adapter")]
fn record_recovered_route_install_obligations(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    mob_id: &crate::MobId,
) {
    use crate::machines::mob_machine as mob_dsl;

    let derived: Vec<mob_dsl::RouteInstallObligation> =
        super::derive_install_obligations(authority.state(), None)
            .into_iter()
            .collect();
    for obligation in derived {
        if let Err(error) = mob_dsl::MobMachineMutator::apply(
            authority,
            mob_dsl::MobMachineInput::RecordRouteInstall {
                obligation: obligation.clone(),
            },
        ) {
            tracing::debug!(
                mob_id = %mob_id,
                host = %obligation.host.as_str(),
                %error,
                "recovery route-install re-derive skipped by machine admission"
            );
        }
    }
}

type PlacedMemberRecoveryKey = (String, u64, crate::ids::PlacedSpawnId);

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlacedCarrierRecoveryDisposition {
    /// A Pending carrier is never membership.  Recovery must first certify
    /// the exact remote attempt absent, then delete the exact carrier.
    PendingCleanup,
    /// A committed carrier with no terminal retirement is the sole recovery
    /// owner for the current placed incarnation.
    CommittedCurrent,
    /// A committed carrier followed by the exact terminal retirement is a
    /// cleanup obligation, never current membership.
    CommittedCleanup,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone)]
struct PlacedCarrierRecoveryEntry {
    record: crate::store::MobPlacedSpawnCarrierRecord,
    disposition: PlacedCarrierRecoveryDisposition,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone, Default)]
struct PlacedCarrierRecoveryPlan {
    entries: Vec<PlacedCarrierRecoveryEntry>,
    current_committed_event_keys: BTreeSet<PlacedMemberRecoveryKey>,
    /// One above every carrier fence, including Pending and carriers that are
    /// synchronously cleaned before the actor is constructed. Zero is the
    /// exhausted sentinel when a durable carrier already consumed u64::MAX.
    next_carrier_fence_token: u64,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone)]
struct PlacedSpawnProjection {
    event_index: usize,
    event: crate::event::MemberSpawnedEvent,
    current_for_exact_generation: bool,
}

#[cfg(feature = "runtime-adapter")]
fn domain_runtime_mode_from_portable(
    mode: meerkat_contracts::wire::WireMobRuntimeMode,
) -> crate::MobRuntimeMode {
    match mode {
        meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost => {
            crate::MobRuntimeMode::AutonomousHost
        }
        meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven => {
            crate::MobRuntimeMode::TurnDriven
        }
    }
}

#[cfg(feature = "runtime-adapter")]
fn domain_continuity_from_portable(
    continuity: &meerkat_contracts::wire::WireSpawnContinuityIntent,
) -> crate::runtime::SpawnContinuityIntent {
    match continuity {
        meerkat_contracts::wire::WireSpawnContinuityIntent::Ephemeral => {
            crate::runtime::SpawnContinuityIntent::Ephemeral
        }
        meerkat_contracts::wire::WireSpawnContinuityIntent::DurableIdentity { continuity_key } => {
            crate::runtime::SpawnContinuityIntent::DurableIdentity {
                continuity_key: continuity_key.clone(),
            }
        }
    }
}

#[cfg(feature = "runtime-adapter")]
fn placed_carrier_cleanup_obligation(
    record: &crate::store::MobPlacedSpawnCarrierRecord,
) -> mob_dsl::PlacedCarrierCleanupObligation {
    mob_dsl::PlacedCarrierCleanupObligation {
        agent_identity: mob_dsl::AgentIdentity::from(record.agent_identity.clone()),
        spawn_id: mob_dsl::PlacedSpawnId(record.spawn_id.to_string()),
        generation: mob_dsl::Generation(record.generation),
        fence_token: mob_dsl::FenceToken(record.fence_token),
        provision_operation_id: record.provision_operation_id.to_string(),
        operation_owner_session_id: mob_dsl::SessionId(
            record.operation_owner_session_id.to_string(),
        ),
        expected_phase: record.expected_phase(),
    }
}

#[cfg(feature = "runtime-adapter")]
fn validate_committed_placed_spawn_projection(
    record: &crate::store::MobPlacedSpawnCarrierRecord,
    event: &crate::event::MemberSpawnedEvent,
) -> Result<(), MobError> {
    let crate::store::PlacedSpawnCarrierPhase::Committed(committed) = &record.phase else {
        return Err(MobError::Internal(format!(
            "Pending placed carrier '{}' unexpectedly reached committed-event validation",
            record.spawn_id
        )));
    };
    let expected_identity = AgentIdentity::from(record.agent_identity.as_str());
    let expected_generation = crate::ids::Generation::new(record.generation);
    let expected_fence = FenceToken::new(record.fence_token);
    let expected_runtime = AgentRuntimeId::new(expected_identity.clone(), expected_generation);
    let expected_role = ProfileName::from(record.spec.profile_name.as_str());
    let expected_runtime_mode = domain_runtime_mode_from_portable(record.spec.profile.runtime_mode);
    let expected_labels = record.spec.overlay.labels.clone().unwrap_or_default();
    let expected_override = record.rehydrated_effective_profile_override()?;
    let expected_model_override = record.rehydrated_effective_model_override();
    let expected_continuity =
        domain_continuity_from_portable(&record.spec.overlay.continuity_intent);
    let endpoint = &committed.member_peer_endpoint;
    let exact_private_link = matches!(
        event.bridge_member_ref(),
        Some(crate::event::MemberRef::BackendPeer {
            peer_id,
            address,
            pubkey,
            session_id: None,
            ..
        }) if peer_id == &endpoint.peer_id.0
            && address == &endpoint.address.0
            && pubkey == &endpoint.signing_key.0
    );
    let mismatch = event.placed_spawn_id() != Some(&record.spawn_id)
        || event.agent_identity != expected_identity
        || event.generation != expected_generation
        || event.fence_token != expected_fence
        || event.agent_runtime_id != expected_runtime
        || event.role != expected_role
        || event.runtime_mode != expected_runtime_mode
        || event.labels != expected_labels
        || event.effective_profile_override != expected_override
        || event.effective_model_override != expected_model_override
        || event.continuity_intent != expected_continuity
        || !exact_private_link;
    if mismatch {
        return Err(MobError::Internal(format!(
            "placed MemberSpawned projection does not exactly match committed carrier spawn_id={} identity={} generation={} fence={}",
            record.spawn_id, record.agent_identity, record.generation, record.fence_token
        )));
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
fn repaired_placed_spawn_event(
    mob_id: &MobId,
    record: &crate::store::MobPlacedSpawnCarrierRecord,
) -> Result<NewMobEvent, MobError> {
    let crate::store::PlacedSpawnCarrierPhase::Committed(committed) = &record.phase else {
        return Err(MobError::Internal(format!(
            "cannot repair MemberSpawned from Pending carrier '{}'",
            record.spawn_id
        )));
    };
    let identity = AgentIdentity::from(record.agent_identity.as_str());
    let generation = crate::ids::Generation::new(record.generation);
    let mut spawned = crate::event::MemberSpawnedEvent::new(
        identity.clone(),
        generation,
        FenceToken::new(record.fence_token),
        AgentRuntimeId::new(identity, generation),
        ProfileName::from(record.spec.profile_name.as_str()),
    )
    .with_bridge_member_ref(Some(crate::event::MemberRef::BackendPeer {
        peer_id: committed.member_peer_endpoint.peer_id.0.clone(),
        address: committed.member_peer_endpoint.address.0.clone(),
        pubkey: committed.member_peer_endpoint.signing_key.0,
        bootstrap_token: None,
        session_id: None,
    }))
    .with_placed_spawn_id(Some(record.spawn_id.clone()));
    spawned.runtime_mode = domain_runtime_mode_from_portable(record.spec.profile.runtime_mode);
    spawned.labels = record.spec.overlay.labels.clone().unwrap_or_default();
    spawned.effective_profile_override = record.rehydrated_effective_profile_override()?;
    spawned.effective_model_override = record.rehydrated_effective_model_override();
    spawned.continuity_intent =
        domain_continuity_from_portable(&record.spec.overlay.continuity_intent);
    Ok(NewMobEvent {
        mob_id: mob_id.clone(),
        timestamp: None,
        kind: MobEventKind::MemberSpawned(spawned),
    })
}

#[cfg(feature = "runtime-adapter")]
fn classify_placed_spawn_recovery(
    mob_id: &MobId,
    events: &[crate::event::MobEvent],
    carriers: &[crate::store::MobPlacedSpawnCarrierRecord],
) -> Result<(PlacedCarrierRecoveryPlan, Vec<NewMobEvent>), MobError> {
    let mut retire_indices = BTreeMap::<(String, u64), Vec<usize>>::new();
    let mut retirement_started_keys = BTreeSet::<(String, u64)>::new();
    for (event_index, event) in events.iter().enumerate() {
        match &event.kind {
            MobEventKind::MemberRetired {
                agent_identity,
                generation,
                ..
            } => {
                retire_indices
                    .entry((agent_identity.as_str().to_string(), generation.get()))
                    .or_default()
                    .push(event_index);
            }
            MobEventKind::MemberRetirementStarted {
                agent_identity,
                generation,
                ..
            } => {
                retirement_started_keys
                    .insert((agent_identity.as_str().to_string(), generation.get()));
            }
            _ => {}
        }
    }

    let mut projections = Vec::new();
    let mut event_by_spawn_id = BTreeMap::<crate::ids::PlacedSpawnId, usize>::new();
    for (event_index, event) in events.iter().enumerate() {
        let MobEventKind::MemberSpawned(spawned) = &event.kind else {
            continue;
        };
        let key = (
            spawned.agent_identity.as_str().to_string(),
            spawned.generation.get(),
        );
        let current_for_exact_generation = !retire_indices
            .get(&key)
            .is_some_and(|indices| indices.iter().any(|retire| *retire > event_index));
        let projection_index = projections.len();
        projections.push(PlacedSpawnProjection {
            event_index,
            event: spawned.clone(),
            current_for_exact_generation,
        });
        if let Some(spawn_id) = spawned.placed_spawn_id()
            && event_by_spawn_id
                .insert(spawn_id.clone(), projection_index)
                .is_some()
        {
            return Err(MobError::Internal(format!(
                "duplicate placed MemberSpawned projection for spawn_id={spawn_id}"
            )));
        }
    }

    let mut carrier_spawn_ids = BTreeSet::new();
    let mut carrier_identities = BTreeSet::new();
    let mut sorted_carriers = carriers.to_vec();
    sorted_carriers.sort_by(|left, right| {
        left.agent_identity
            .cmp(&right.agent_identity)
            .then_with(|| left.generation.cmp(&right.generation))
            .then_with(|| left.spawn_id.cmp(&right.spawn_id))
    });
    let mut plan = PlacedCarrierRecoveryPlan::default();
    let mut repairs = Vec::new();
    let mut max_carrier_fence = 0_u64;

    for record in &sorted_carriers {
        record.validate_for_mob(mob_id)?;
        max_carrier_fence = max_carrier_fence.max(record.fence_token);
        if !carrier_identities.insert(record.agent_identity.clone()) {
            return Err(MobError::Internal(format!(
                "duplicate placed-spawn carrier identity '{}' during recovery",
                record.agent_identity
            )));
        }
        if !carrier_spawn_ids.insert(record.spawn_id.clone()) {
            return Err(MobError::Internal(format!(
                "duplicate placed-spawn carrier id '{}' during recovery",
                record.spawn_id
            )));
        }
    }

    for record in sorted_carriers {
        let exact_projection = event_by_spawn_id
            .get(&record.spawn_id)
            .map(|index| &projections[*index]);
        if let Some(projection) = exact_projection
            && (projection.event.agent_identity.as_str() != record.agent_identity
                || projection.event.generation.get() != record.generation)
        {
            return Err(MobError::Internal(format!(
                "placed spawn_id={} is bound to carrier tuple {}:{} but event tuple is {}:{}",
                record.spawn_id,
                record.agent_identity,
                record.generation,
                projection.event.agent_identity,
                projection.event.generation.get()
            )));
        }
        let exact_key = (record.agent_identity.clone(), record.generation);
        if let Some(projection) = exact_projection
            && retire_indices.get(&exact_key).is_some_and(|indices| {
                indices
                    .iter()
                    .any(|retire| *retire < projection.event_index)
            })
        {
            return Err(MobError::Internal(format!(
                "placed MemberSpawned spawn_id={} resurrects retired identity '{}' generation={}",
                record.spawn_id, record.agent_identity, record.generation
            )));
        }
        let any_projectable_exact_tuple = projections.iter().any(|projection| {
            projection.event.agent_identity.as_str() == record.agent_identity
                && projection.event.generation.get() == record.generation
                && projection.event.bridge_member_ref().is_some()
        });
        let current_projectable_same_identity = projections
            .iter()
            .filter(|projection| {
                projection.current_for_exact_generation
                    && projection.event.agent_identity.as_str() == record.agent_identity
                    && projection.event.bridge_member_ref().is_some()
            })
            .collect::<Vec<_>>();
        let terminal_retirement = if let Some(projection) = exact_projection {
            retire_indices.get(&exact_key).is_some_and(|indices| {
                indices
                    .iter()
                    .any(|retire| *retire > projection.event_index)
            })
        } else {
            false
        };

        let disposition = match &record.phase {
            crate::store::PlacedSpawnCarrierPhase::Pending => {
                // Event-before-carrier-commit is a phantom membership.  Even a
                // later retirement cannot turn a never-committed attempt into
                // valid history, and an untagged exact-tuple projection is not
                // sufficient to prove a different attempt.
                if exact_projection.is_some() || any_projectable_exact_tuple {
                    return Err(MobError::Internal(format!(
                        "Pending placed carrier spawn_id={} identity={} generation={} has a projectable MemberSpawned event",
                        record.spawn_id, record.agent_identity, record.generation
                    )));
                }
                PlacedCarrierRecoveryDisposition::PendingCleanup
            }
            crate::store::PlacedSpawnCarrierPhase::Committed(_) => {
                if exact_projection.is_none() && retirement_started_keys.contains(&exact_key) {
                    return Err(MobError::Internal(format!(
                        "committed placed carrier spawn_id={} has MemberRetirementStarted but no exact placed MemberSpawned link",
                        record.spawn_id
                    )));
                }
                if exact_projection.is_none() && retire_indices.contains_key(&exact_key) {
                    return Err(MobError::Internal(format!(
                        "committed placed carrier spawn_id={} has a same-generation retirement but no exact placed MemberSpawned link",
                        record.spawn_id
                    )));
                }
                if let Some(projection) = exact_projection {
                    validate_committed_placed_spawn_projection(&record, &projection.event)?;
                }
                if terminal_retirement {
                    PlacedCarrierRecoveryDisposition::CommittedCleanup
                } else if let Some(projection) = exact_projection {
                    if !projection.current_for_exact_generation
                        || current_projectable_same_identity.len() != 1
                    {
                        return Err(MobError::Internal(format!(
                            "committed placed carrier spawn_id={} has conflicting current MemberSpawned projections for identity '{}'",
                            record.spawn_id, record.agent_identity
                        )));
                    }
                    plan.current_committed_event_keys.insert((
                        record.agent_identity.clone(),
                        record.generation,
                        record.spawn_id.clone(),
                    ));
                    PlacedCarrierRecoveryDisposition::CommittedCurrent
                } else {
                    if !current_projectable_same_identity.is_empty() {
                        return Err(MobError::Internal(format!(
                            "committed placed carrier spawn_id={} is missing its exact event but identity '{}' already has a current projectable MemberSpawned",
                            record.spawn_id, record.agent_identity
                        )));
                    }
                    repairs.push(repaired_placed_spawn_event(mob_id, &record)?);
                    PlacedCarrierRecoveryDisposition::CommittedCurrent
                }
            }
        };
        plan.entries.push(PlacedCarrierRecoveryEntry {
            record,
            disposition,
        });
    }

    for projection in &projections {
        let Some(spawn_id) = projection.event.placed_spawn_id() else {
            continue;
        };
        if projection.current_for_exact_generation && !carrier_spawn_ids.contains(spawn_id) {
            return Err(MobError::Internal(format!(
                "current placed MemberSpawned spawn_id={} identity={} generation={} has no exact carrier",
                spawn_id,
                projection.event.agent_identity,
                projection.event.generation.get()
            )));
        }
    }

    plan.next_carrier_fence_token = max_carrier_fence.checked_add(1).unwrap_or(0);
    Ok((plan, repairs))
}

#[cfg(feature = "runtime-adapter")]
fn validate_placed_carrier_owner_bindings(
    authority: &crate::machines::mob_machine::MobMachineAuthority,
    plan: &PlacedCarrierRecoveryPlan,
) -> Result<(), MobError> {
    let recovered_owner = authority.state().owner_bridge_session_id.as_ref();
    for entry in &plan.entries {
        if recovered_owner
            .is_none_or(|owner| owner.0 != entry.record.operation_owner_session_id.to_string())
        {
            return Err(MobError::Internal(format!(
                "placed carrier spawn_id={} operation owner '{}' does not match recovered owner bridge {:?}",
                entry.record.spawn_id,
                entry.record.operation_owner_session_id,
                recovered_owner.map(|owner| owner.0.as_str())
            )));
        }
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
fn validate_placed_spawn_repair_prerequisites(
    authority: &crate::machines::mob_machine::MobMachineAuthority,
    plan: &PlacedCarrierRecoveryPlan,
) -> Result<(), MobError> {
    validate_placed_carrier_owner_bindings(authority, plan)?;
    if !plan
        .entries
        .iter()
        .any(|entry| entry.disposition == PlacedCarrierRecoveryDisposition::CommittedCurrent)
    {
        return Ok(());
    }
    let Some(recovered_owner) = authority.state().owner_bridge_session_id.as_ref() else {
        return Err(MobError::Internal(
            "cannot repair a committed placed MemberSpawned event before owner bridge authority is recovered"
                .to_string(),
        ));
    };
    for entry in &plan.entries {
        if entry.disposition != PlacedCarrierRecoveryDisposition::CommittedCurrent {
            continue;
        }
        if recovered_owner.0 != entry.record.operation_owner_session_id.to_string() {
            return Err(MobError::Internal(format!(
                "cannot repair committed placed MemberSpawned spawn_id={}: carrier operation owner '{}' does not match recovered owner bridge '{}'",
                entry.record.spawn_id, entry.record.operation_owner_session_id, recovered_owner.0
            )));
        }
        // The logical committed member survives a revoked host binding. Its
        // carrier generation remains dormant until authenticated replacement
        // materialization/status promotes it through the exact CAS lane.
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
async fn prepare_placed_spawn_recovery(
    runtime_metadata: &Arc<dyn crate::store::MobRuntimeMetadataStore>,
    mob_id: &MobId,
    mob_events: &[crate::event::MobEvent],
) -> Result<(PlacedCarrierRecoveryPlan, Vec<NewMobEvent>), MobError> {
    // `list_placed_spawns` validates every decoded row.  The explicit
    // per-record validation in the classifier also protects test/future store
    // implementations from returning a row under the wrong mob partition.
    let carriers = runtime_metadata.list_placed_spawns(mob_id).await?;
    let initial_epoch_start = mob_events
        .iter()
        .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
        .map_or(0, |position| position + 1);
    let destroy_storage_finalizing = mob_events[initial_epoch_start..]
        .iter()
        .any(|event| matches!(event.kind, MobEventKind::MobDestroyStorageFinalizing));
    if destroy_storage_finalizing && !carriers.is_empty() {
        return Err(MobError::Internal(format!(
            "cannot resume mob '{mob_id}': destroy storage finalization is durable but {} placed-spawn carrier(s) remain",
            carriers.len()
        )));
    }
    let epoch_start = mob_events
        .iter()
        .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
        .map_or(0, |position| position + 1);
    classify_placed_spawn_recovery(mob_id, &mob_events[epoch_start..], &carriers)
}

#[cfg(feature = "runtime-adapter")]
fn validate_repaired_placed_spawn_machine_authority(
    authority: &crate::machines::mob_machine::MobMachineAuthority,
    entry: &PlacedCarrierRecoveryEntry,
) -> Result<(), MobError> {
    validate_placed_spawn_repair_prerequisites(
        authority,
        &PlacedCarrierRecoveryPlan {
            entries: vec![entry.clone()],
            ..PlacedCarrierRecoveryPlan::default()
        },
    )?;
    let identity = mob_dsl::AgentIdentity::from(entry.record.agent_identity.clone());
    let expected_spawn_id = mob_dsl::PlacedSpawnId(entry.record.spawn_id.to_string());
    let expected_operation_id = entry.record.provision_operation_id.to_string();
    let expected_owner = mob_dsl::SessionId(entry.record.operation_owner_session_id.to_string());
    let state = authority.state();
    if state.current_placed_spawn_ids.get(&identity) != Some(&expected_spawn_id)
        || state
            .current_placed_spawn_host_binding_generations
            .get(&identity)
            .copied()
            != Some(entry.record.host_binding_generation)
        || state
            .current_placed_spawn_provision_operation_ids
            .get(&identity)
            != Some(&expected_operation_id)
        || state
            .current_placed_spawn_operation_owner_session_ids
            .get(&identity)
            != Some(&expected_owner)
    {
        return Err(MobError::Internal(format!(
            "cannot repair committed placed MemberSpawned spawn_id={}: exact RecoverCommittedPlacedSpawn machine authority is absent",
            entry.record.spawn_id
        )));
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
#[allow(clippy::too_many_arguments)]
async fn commit_placed_spawn_event_repairs(
    event_store: &Arc<dyn crate::store::MobEventStore>,
    runtime_metadata: &Arc<dyn crate::store::MobRuntimeMetadataStore>,
    mob_id: &MobId,
    mob_events: &mut Vec<crate::event::MobEvent>,
    initial_plan: PlacedCarrierRecoveryPlan,
    repairs: Vec<NewMobEvent>,
    authority: &crate::machines::mob_machine::MobMachineAuthority,
    provisioner: &dyn MobProvisioner,
) -> Result<PlacedCarrierRecoveryPlan, MobError> {
    if repairs.is_empty() {
        return Ok(initial_plan);
    }

    // A repaired MemberSpawned is a new public durability claim.  The carrier
    // first has to survive exact MobMachine recovery, then the operation
    // registry must be Running under the carrier's pre-minted id and owner.
    // A real process restart may have lost the nonterminal registry row; the
    // provisioner reconstructs that SAME id/full tuple rather than discovering
    // or minting a replacement.
    let repair_spawn_ids = repairs
        .iter()
        .map(|repair| match &repair.kind {
            MobEventKind::MemberSpawned(spawned) => {
                spawned.placed_spawn_id().cloned().ok_or_else(|| {
                    MobError::Internal("placed-spawn repair lacks its private spawn id".to_string())
                })
            }
            _ => Err(MobError::Internal(
                "placed-spawn repair contains a non-MemberSpawned event".to_string(),
            )),
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    for spawn_id in &repair_spawn_ids {
        let entry = initial_plan
            .entries
            .iter()
            .find(|entry| &entry.record.spawn_id == spawn_id)
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "placed-spawn repair references unknown carrier spawn_id={spawn_id}"
                ))
            })?;
        if entry.disposition != PlacedCarrierRecoveryDisposition::CommittedCurrent {
            return Err(MobError::Internal(format!(
                "placed-spawn repair references non-current carrier spawn_id={spawn_id}"
            )));
        }
        validate_repaired_placed_spawn_machine_authority(authority, entry)?;
        let display_name = super::actor::render_member_comms_name(
            mob_id.as_str(),
            &entry.record.spec.profile_name,
            &entry.record.agent_identity,
        )?;
        provisioner
            .ensure_committed_placed_provision_operation(
                &entry.record.operation_owner_session_id,
                &entry.record.provision_operation_id,
                &display_name,
            )
            .await?;
    }

    event_store.append_batch(repairs).await?;
    *mob_events = event_store
        .replay_all()
        .await?
        .into_iter()
        .filter(|event| event.mob_id == *mob_id)
        .collect();

    // Reload both sides and require deterministic convergence. Preserve the
    // original allocator seed, including its zero exhaustion sentinel,
    // because cleanup records may already have been deleted between
    // classification and this reload.
    let (mut reloaded_plan, second_repairs) =
        prepare_placed_spawn_recovery(runtime_metadata, mob_id, mob_events).await?;
    if !second_repairs.is_empty() {
        return Err(MobError::Internal(format!(
            "placed-spawn event repair for mob '{mob_id}' did not converge after reload"
        )));
    }
    reloaded_plan.next_carrier_fence_token = combine_recovered_fence_token_seeds(
        reloaded_plan.next_carrier_fence_token,
        initial_plan.next_carrier_fence_token,
    );
    Ok(reloaded_plan)
}

fn seed_mob_authority_sync_from_events(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    events: &[crate::event::MobEvent],
    definition: &MobDefinition,
    defer_completed_phase: bool,
    committed_placed_recovery_keys: &BTreeSet<PlacedMemberRecoveryKey>,
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;

    let respawn_topology_ledger = respawn_topology_ledger(events)?;

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
                if !defer_completed_phase {
                    if authority.state().placed_completion_lifecycle_intent
                        != Some(mob_dsl::PlacedCompletionLifecycleIntentKind::Complete)
                    {
                        apply_seeded_mob_input(
                            authority,
                            mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
                                intent: mob_dsl::PlacedCompletionLifecycleIntentKind::Complete,
                            },
                            "recover_legacy_mob_completed_quiesce",
                        )?;
                    }
                    apply_seeded_mob_input(
                        authority,
                        mob_dsl::MobMachineInput::Complete,
                        "recover_mob_completed",
                    )?;
                }
            }
            MobEventKind::MobDestroying => {
                if authority.state().placed_completion_lifecycle_intent
                    != Some(mob_dsl::PlacedCompletionLifecycleIntentKind::Destroy)
                {
                    apply_seeded_mob_input(
                        authority,
                        mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
                            intent: mob_dsl::PlacedCompletionLifecycleIntentKind::Destroy,
                        },
                        "recover_legacy_destroy_admitted_quiesce",
                    )?;
                }
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::AdmitDestroyCleanup,
                    "recover_destroy_admitted",
                )?;
            }
            MobEventKind::MobDestroyStorageFinalizing => {
                if authority.state().placed_completion_lifecycle_intent
                    != Some(mob_dsl::PlacedCompletionLifecycleIntentKind::Destroy)
                {
                    apply_seeded_mob_input(
                        authority,
                        mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
                            intent: mob_dsl::PlacedCompletionLifecycleIntentKind::Destroy,
                        },
                        "recover_legacy_destroy_finalizing_quiesce",
                    )?;
                }
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
            // Completion custody recovery replays these after every exact
            // Record/cancel/resolved chain has been reconstructed. Applying
            // Begin here would close the fresh-Record recovery gate too soon.
            MobEventKind::PlacedCompletionLifecycleQuiesceStarted { .. }
            | MobEventKind::PlacedCompletionLifecycleQuiesceEnded { .. }
            | MobEventKind::MobStopped => {}
            MobEventKind::MemberSpawned(member_spawned) => {
                let recovery_key = member_spawned.placed_spawn_id().map(|spawn_id| {
                    (
                        member_spawned.agent_identity.as_str().to_string(),
                        member_spawned.generation.get(),
                        spawn_id.clone(),
                    )
                });
                if recovery_key
                    .as_ref()
                    .is_some_and(|key| committed_placed_recovery_keys.contains(key))
                {
                    // A committed placed-spec record is the one cold-recovery
                    // owner for the current remote incarnation's membership,
                    // session, peer endpoint, and placement. Replaying this
                    // event first would install generic membership and make
                    // the exact remote spawn ladder reject its own durable
                    // carrier. The key is incarnation-scoped so older
                    // generations still replay and retain history.
                    continue;
                }
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
                if member_spawned.placed_spawn_id().is_none()
                    && let Some(member_peer_endpoint) = member_spawned.member_peer_endpoint()
                {
                    // The endpoint is bound to this exact MemberSpawned
                    // generation/fence/runtime tuple in the lifecycle journal.
                    // Restore local and peer-only MobMachine endpoint authority
                    // before any wiring reconciliation; a broken session may
                    // have no live comms or surviving trust projection to
                    // rediscover it. Placed endpoints stay carrier-owned and
                    // are recovered only by the exact placed-spawn ladder.
                    apply_seeded_mob_signal(
                        authority,
                        mob_dsl::MobMachineSignal::RecoverSpawnedMemberPeerEndpoint {
                            agent_identity: mob_dsl::AgentIdentity::from_domain(
                                &member_spawned.agent_identity,
                            ),
                            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                                &member_spawned.agent_runtime_id,
                            ),
                            fence_token: mob_dsl::FenceToken::from_domain(
                                member_spawned.fence_token,
                            ),
                            generation: mob_dsl::Generation::from_domain(member_spawned.generation),
                            peer_endpoint: mob_dsl::MemberPeerEndpoint::from(member_peer_endpoint),
                        },
                        "recover_member_spawned_peer_endpoint",
                    )?;
                }
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
                new_generation,
                fence_token,
                agent_runtime_id,
                ..
            } => {
                if &agent_runtime_id.identity != agent_identity {
                    return Err(MobError::Internal(format!(
                        "stored MemberReset identity '{}' does not match runtime identity '{}'",
                        agent_identity, agent_runtime_id.identity
                    )));
                }
                if *new_generation != agent_runtime_id.generation {
                    return Err(MobError::Internal(format!(
                        "stored MemberReset new generation '{}' does not match runtime generation '{}' for '{}'",
                        new_generation, agent_runtime_id.generation, agent_identity
                    )));
                }
                let expected_generation = previous_generation
                    .get()
                    .checked_add(1)
                    .map(crate::ids::Generation::new)
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "stored MemberReset previous generation overflow for '{agent_identity}'"
                        ))
                    })?;
                if *new_generation != expected_generation {
                    return Err(MobError::Internal(format!(
                        "stored MemberReset generation '{new_generation}' is not the exact successor of '{previous_generation}' for '{agent_identity}'"
                    )));
                }
                let previous_agent_runtime_id =
                    AgentRuntimeId::new(agent_identity.clone(), *previous_generation);
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRosterMemberReset {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        previous_agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                            &previous_agent_runtime_id,
                        ),
                        previous_generation: mob_dsl::Generation::from_domain(*previous_generation),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
                        fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
                        generation: mob_dsl::Generation::from_domain(*new_generation),
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
                if let Some(member_peer_endpoint) = recovered.member_peer_endpoint.as_ref() {
                    // This endpoint is authorized by the durable session-head
                    // recovery for the exact runtime/session binding above.
                    // Unlike an uncorrelated live observation, it may replace
                    // the spawn endpoint without weakening mismatch checks.
                    apply_seeded_mob_signal(
                        authority,
                        mob_dsl::MobMachineSignal::RecoverMemberPeerEndpoint {
                            agent_identity: mob_dsl::AgentIdentity::from_domain(
                                &recovered.agent_identity,
                            ),
                            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                                &recovered.agent_runtime_id,
                            ),
                            bridge_session_id: mob_dsl::SessionId::from_domain(bridge_session_id),
                            peer_endpoint: mob_dsl::MemberPeerEndpoint::from(member_peer_endpoint),
                        },
                        "recover_member_session_binding_peer_endpoint",
                    )?;
                }
            }
            // Retirement start is deliberately replayed in a second pass.
            // A placed member's exact session, endpoint, and placement facts
            // live in the committed host-placement carrier, which is restored
            // only after this general lifecycle pass. Applying Retiring here
            // would therefore either reject a truthful placed Started event
            // or weaken the signal's exact-carrier guards.
            MobEventKind::MemberRetirementStarted { .. } => {}
            MobEventKind::MemberRetired {
                agent_identity,
                generation,
                ..
            } => {
                let agent_runtime_id = AgentRuntimeId::new(agent_identity.clone(), *generation);
                let key = (agent_identity.as_str().to_string(), generation.get());
                let preserve_machine_topology = respawn_topology_ledger
                    .effective_preservation
                    .get(&key)
                    .copied()
                    .unwrap_or(false);
                let preservation_started = respawn_topology_ledger
                    .starts
                    .get(&key)
                    .is_some_and(|start| start.preserve_machine_topology);
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRosterMemberRetired {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&agent_runtime_id),
                        generation: mob_dsl::Generation::from_domain(*generation),
                        preserve_machine_topology,
                        preservation_started,
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
                if let Some(objective_id) = kickoff.objective_id {
                    if authority.state().implicit_delegation_mob
                        && let Some(owner_session_id) =
                            authority.state().owner_bridge_session_id.as_ref()
                    {
                        let owner_session_id = SessionId::parse(&owner_session_id.0).map_err(
                            |error| {
                                MobError::Internal(format!(
                                    "implicit delegation recovery has invalid owner bridge session '{}': {error}",
                                    owner_session_id.0
                                ))
                            },
                        )?;
                        let owner_identity =
                            AgentIdentity::objective_lead_for_session(&owner_session_id);
                        apply_seeded_mob_input(
                            authority,
                            mob_dsl::MobMachineInput::BindObjectiveOwner {
                                owner_id: mob_dsl::AgentIdentity::from_domain(&owner_identity),
                                objective_id: objective_id.to_string(),
                            },
                            "recover_implicit_objective_owner",
                        )?;
                    }
                    apply_seeded_mob_signal(
                        authority,
                        mob_dsl::MobMachineSignal::RecoverObjectiveBinding {
                            member_id: mob_dsl::AgentIdentity::from_domain(member),
                            objective_id: objective_id.to_string(),
                        },
                        "recover_objective_binding",
                    )?;
                }
            }
            MobEventKind::ObjectiveOwnerBound {
                owner,
                objective_id,
            } => {
                apply_seeded_mob_input(
                    authority,
                    mob_dsl::MobMachineInput::BindObjectiveOwner {
                        owner_id: mob_dsl::AgentIdentity::from_domain(owner),
                        objective_id: objective_id.to_string(),
                    },
                    "recover_objective_owner",
                )?;
            }
            MobEventKind::ObjectiveConcluded {
                member,
                objective_id,
                outcome,
            } => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverObjectiveConclusion {
                        member_id: mob_dsl::AgentIdentity::from_domain(member),
                        objective_id: objective_id.to_string(),
                        outcome: outcome.clone(),
                    },
                    "recover_objective_conclusion",
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
    seed_mob_definition_spawn_policy(authority, definition, "recover_definition_spawn_policy")?;
    Ok(())
}

/// Recover the exact respawn-topology intent carried by each retirement
/// incarnation. The carrier is immutable once durable: byte-for-byte duplicate
/// starts are tolerated for legacy retry compatibility, but a second start
/// that changes any field (especially the preservation bit) is corrupt rather
/// than a last-write-wins upgrade.
#[derive(Clone)]
struct RespawnTopologyStart {
    agent_identity: AgentIdentity,
    agent_runtime_id: AgentRuntimeId,
    generation: crate::ids::Generation,
    preserve_machine_topology: bool,
    cursor: u64,
    kind: MobEventKind,
}

struct RespawnTopologyLedger {
    starts: BTreeMap<(String, u64), RespawnTopologyStart>,
    effective_preservation: BTreeMap<(String, u64), bool>,
    active_abandonments: BTreeMap<(String, u64), (AgentRuntimeId, crate::ids::FenceToken, u64)>,
    spawned: BTreeMap<(String, u64), (AgentRuntimeId, crate::ids::FenceToken, u64)>,
}

fn respawn_topology_ledger(
    events: &[crate::event::MobEvent],
) -> Result<RespawnTopologyLedger, MobError> {
    let mut starts = BTreeMap::<(String, u64), RespawnTopologyStart>::new();
    let mut spawned =
        BTreeMap::<(String, u64), (AgentRuntimeId, crate::ids::FenceToken, u64)>::new();
    for event in events {
        match &event.kind {
            MobEventKind::MemberSpawned(member) => {
                let key = (
                    member.agent_identity.as_str().to_string(),
                    member.generation.get(),
                );
                let tuple = (
                    member.agent_runtime_id.clone(),
                    member.fence_token,
                    event.cursor,
                );
                if let Some(prior) = spawned.get(&key) {
                    if prior.0 != tuple.0 || prior.1 != tuple.1 {
                        return Err(MobError::Internal(format!(
                            "conflicting MemberSpawned carriers for '{}' generation {}",
                            member.agent_identity, member.generation
                        )));
                    }
                } else {
                    spawned.insert(key, tuple);
                }
            }
            MobEventKind::MemberRetirementStarted {
                agent_identity,
                agent_runtime_id,
                generation,
                preserve_machine_topology,
                ..
            } => {
                if &agent_runtime_id.identity != agent_identity
                    || agent_runtime_id.generation != *generation
                {
                    return Err(MobError::Internal(format!(
                        "stored MemberRetirementStarted incarnation mismatch for '{}' at cursor {}",
                        agent_identity, event.cursor
                    )));
                }
                let key = (agent_identity.as_str().to_string(), generation.get());
                if let Some(prior) = starts.get(&key) {
                    if prior.kind != event.kind {
                        return Err(MobError::Internal(format!(
                            "conflicting MemberRetirementStarted carriers for '{}' generation {} at cursors {} and {}",
                            agent_identity, generation, prior.cursor, event.cursor
                        )));
                    }
                } else {
                    starts.insert(
                        key,
                        RespawnTopologyStart {
                            agent_identity: agent_identity.clone(),
                            agent_runtime_id: agent_runtime_id.clone(),
                            generation: *generation,
                            preserve_machine_topology: *preserve_machine_topology,
                            cursor: event.cursor,
                            kind: event.kind.clone(),
                        },
                    );
                }
            }
            _ => {}
        }
    }

    let mut abandonment_candidates =
        BTreeMap::<(String, u64), (AgentRuntimeId, crate::ids::FenceToken, u64)>::new();
    for event in events {
        let MobEventKind::RespawnTopologyAbandoned {
            agent_identity,
            generation,
            agent_runtime_id,
            fence_token,
        } = &event.kind
        else {
            continue;
        };
        let agent_runtime_id = agent_runtime_id.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "stored RespawnTopologyAbandoned for '{agent_identity}' generation {generation} omitted agent_runtime_id"
            ))
        })?;
        let fence_token = fence_token.ok_or_else(|| {
            MobError::Internal(format!(
                "stored RespawnTopologyAbandoned for '{agent_identity}' generation {generation} omitted fence_token"
            ))
        })?;
        if agent_runtime_id.identity != *agent_identity
            || agent_runtime_id.generation != *generation
        {
            return Err(MobError::Internal(format!(
                "stored RespawnTopologyAbandoned incarnation mismatch for '{}' generation {} at cursor {}",
                agent_identity, generation, event.cursor
            )));
        }
        let key = (agent_identity.as_str().to_string(), generation.get());
        let start = starts.get(&key).ok_or_else(|| {
            MobError::Internal(format!(
                "RespawnTopologyAbandoned for '{agent_identity}' generation {generation} has no preceding retirement start"
            ))
        })?;
        if !start.preserve_machine_topology || start.cursor >= event.cursor {
            return Err(MobError::Internal(format!(
                "RespawnTopologyAbandoned for '{agent_identity}' generation {generation} is not ordered after an exact preserving retirement start"
            )));
        }
        let Some((spawn_runtime_id, spawn_fence_token, spawn_cursor)) = spawned.get(&key) else {
            return Err(MobError::Internal(format!(
                "RespawnTopologyAbandoned for '{agent_identity}' generation {generation} has no exact spawned incarnation"
            )));
        };
        if spawn_runtime_id != agent_runtime_id
            || *spawn_fence_token != fence_token
            || *spawn_cursor >= start.cursor
        {
            return Err(MobError::Internal(format!(
                "RespawnTopologyAbandoned exact tuple for '{agent_identity}' generation {generation} conflicts with its spawned incarnation"
            )));
        }
        let tuple = (agent_runtime_id.clone(), fence_token, event.cursor);
        if let Some(prior) = abandonment_candidates.get(&key) {
            if prior.0 != tuple.0 || prior.1 != tuple.1 {
                return Err(MobError::Internal(format!(
                    "conflicting RespawnTopologyAbandoned carriers for '{agent_identity}' generation {generation}"
                )));
            }
        } else {
            abandonment_candidates.insert(key, tuple);
        }
    }

    let mut active_abandonments = BTreeMap::new();
    for (key, tuple) in abandonment_candidates {
        let stale_after_successor =
            spawned
                .iter()
                .any(|((identity, generation), (_, _, cursor))| {
                    identity == &key.0 && *generation > key.1 && *cursor < tuple.2
                });
        if !stale_after_successor {
            active_abandonments.insert(key, tuple);
        }
    }

    for event in events {
        let MobEventKind::MemberRetired {
            agent_identity,
            generation,
            ..
        } = &event.kind
        else {
            continue;
        };
        let key = (agent_identity.as_str().to_string(), generation.get());
        if let Some(start) = starts.get(&key)
            && start.preserve_machine_topology
            && start.cursor >= event.cursor
        {
            return Err(MobError::Internal(format!(
                "preserving MemberRetirementStarted for '{}' generation {} must precede terminal cursor {}",
                agent_identity, generation, event.cursor
            )));
        }
    }

    let effective_preservation = starts
        .iter()
        .map(|(key, start)| {
            (
                key.clone(),
                start.preserve_machine_topology && !active_abandonments.contains_key(key),
            )
        })
        .collect();
    Ok(RespawnTopologyLedger {
        starts,
        effective_preservation,
        active_abandonments,
        spawned,
    })
}

/// A respawn helper is not itself a durable replacement operation. If replay
/// finds a preserving retirement start without an exact successor, write the
/// generation-scoped abandonment marker before the actor is exposed. For an
/// in-flight retirement the marker switches future cleanup to ordinary while
/// retaining graph authority; for a terminal gap replay prunes the retained
/// graph atomically through the same marker semantics.
async fn ensure_recovered_respawn_topology_abandonments(
    event_store: &(dyn MobEventStore + 'static),
    mob_id: &MobId,
    mob_events: &mut Vec<crate::event::MobEvent>,
    committed_placed_recovery_keys: &BTreeSet<PlacedMemberRecoveryKey>,
) -> Result<(), MobError> {
    let epoch_start = mob_events
        .iter()
        .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
        .map_or(0, |index| index + 1);
    let ledger = respawn_topology_ledger(&mob_events[epoch_start..])?;
    let mut desired = Vec::new();
    for (key, start) in &ledger.starts {
        if !start.preserve_machine_topology || ledger.active_abandonments.contains_key(key) {
            continue;
        }
        let event_successor = ledger
            .spawned
            .keys()
            .any(|(identity, generation)| identity == &key.0 && *generation > key.1);
        let placed_successor = committed_placed_recovery_keys
            .iter()
            .any(|(identity, generation, _)| identity == &key.0 && *generation > key.1);
        if event_successor || placed_successor {
            continue;
        }
        let Some((spawn_runtime_id, spawn_fence_token, spawn_cursor)) = ledger.spawned.get(key)
        else {
            return Err(MobError::Internal(format!(
                "preserving retirement start for '{}' generation {} has no exact spawned incarnation",
                start.agent_identity, start.generation
            )));
        };
        if spawn_runtime_id != &start.agent_runtime_id || *spawn_cursor >= start.cursor {
            return Err(MobError::Internal(format!(
                "preserving retirement start for '{}' generation {} conflicts with its spawned incarnation",
                start.agent_identity, start.generation
            )));
        }
        desired.push(MobEventKind::RespawnTopologyAbandoned {
            agent_identity: start.agent_identity.clone(),
            generation: start.generation,
            agent_runtime_id: Some(start.agent_runtime_id.clone()),
            fence_token: Some(*spawn_fence_token),
        });
    }
    if desired.is_empty() {
        return Ok(());
    }

    let batch = desired
        .iter()
        .cloned()
        .map(|kind| NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind,
        })
        .collect();
    match event_store.append_batch(batch).await {
        Ok(stored) => mob_events.extend(stored),
        Err(append_error) => {
            // Batch append is all-or-nothing. A wrote-then-error backend is
            // accepted only when replay proves every exact marker in this
            // reset epoch; otherwise resume fails closed before actor startup.
            let replayed = event_store.replay_all().await?;
            let replayed_for_mob = replayed
                .into_iter()
                .filter(|event| event.mob_id == *mob_id)
                .collect::<Vec<_>>();
            let replayed_epoch_start = replayed_for_mob
                .iter()
                .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
                .map_or(0, |index| index + 1);
            let exact = desired.iter().all(|desired_kind| {
                replayed_for_mob[replayed_epoch_start..]
                    .iter()
                    .any(|event| event.kind == *desired_kind)
            });
            if !exact {
                return Err(MobError::Internal(format!(
                    "recovered respawn-topology abandonment append failed and exact markers were not durable: {append_error}"
                )));
            }
            *mob_events = replayed_for_mob;
        }
    }
    Ok(())
}

/// Replay only still-pending retirement anchors and their exact remote
/// checkpoints after every member's placement/session carrier has been
/// restored. A completed generation does not need these markers re-applied:
/// `RecoverRosterMemberRetired` already folded the terminal carrier during the
/// general lifecycle pass.
fn recover_pending_member_retirements(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    events: &[crate::event::MobEvent],
) -> Result<(), MobError> {
    use crate::machines::mob_machine as mob_dsl;

    // Validate the full stream here as well as in the general seeding pass.
    // Unit-level recovery callers and future alternate resume entrypoints must
    // never regain last-write-wins semantics for the durable preservation bit.
    let ledger = respawn_topology_ledger(events)?;

    let completed = events
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::MemberRetired {
                agent_identity,
                generation,
                ..
            } => Some((agent_identity.as_str().to_string(), generation.get())),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut recovered_starts = BTreeSet::new();
    for event in events {
        match &event.kind {
            MobEventKind::MemberRetirementStarted {
                agent_identity,
                agent_runtime_id,
                generation,
                releasing,
                session_id,
                retiring_peer_endpoint,
                preserve_machine_topology,
                ..
            } => {
                let key = (agent_identity.as_str().to_string(), generation.get());
                if completed.contains(&key) || !recovered_starts.insert(key) {
                    continue;
                }
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
                        preserve_machine_topology: *preserve_machine_topology,
                    },
                    "recover_pending_member_retirement_started",
                )?;
            }
            MobEventKind::RespawnTopologyAbandoned {
                agent_identity,
                generation,
                agent_runtime_id,
                fence_token,
            } => {
                let key = (agent_identity.as_str().to_string(), generation.get());
                if !ledger.active_abandonments.contains_key(&key) {
                    continue;
                }
                let agent_runtime_id = agent_runtime_id.as_ref().ok_or_else(|| {
                    MobError::Internal(format!(
                        "pending RespawnTopologyAbandoned for '{agent_identity}' omitted agent_runtime_id"
                    ))
                })?;
                let fence_token = fence_token.ok_or_else(|| {
                    MobError::Internal(format!(
                        "pending RespawnTopologyAbandoned for '{agent_identity}' omitted fence_token"
                    ))
                })?;
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::ObserveRespawnTopologyAbandoned {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
                        fence_token: mob_dsl::FenceToken::from_domain(fence_token),
                        generation: mob_dsl::Generation::from_domain(*generation),
                    },
                    "recover_pending_respawn_topology_abandoned",
                )?;
            }
            MobEventKind::RemoteMemberRuntimeRetired {
                agent_identity,
                agent_runtime_id,
                fence_token,
                generation,
            } => {
                let key = (agent_identity.as_str().to_string(), generation.get());
                if completed.contains(&key) {
                    continue;
                }
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRemoteMemberRuntimeRetired {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
                        fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
                        generation: mob_dsl::Generation::from_domain(*generation),
                    },
                    "recover_pending_remote_member_runtime_retired",
                )?;
            }
            MobEventKind::RemoteMemberSupervisorRevoked {
                agent_identity,
                agent_runtime_id,
                fence_token,
                generation,
            } => {
                let key = (agent_identity.as_str().to_string(), generation.get());
                if completed.contains(&key) {
                    continue;
                }
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverRemoteMemberSupervisorRevoked {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id),
                        fence_token: mob_dsl::FenceToken::from_domain(*fence_token),
                        generation: mob_dsl::Generation::from_domain(*generation),
                    },
                    "recover_pending_remote_member_supervisor_revoked",
                )?;
            }
            _ => {}
        }
    }

    // Wiring events record historical intent. Converge only after current
    // placed carriers and terminal retirement events have established which
    // identity incarnations survived replay; doing this during the first pass
    // would incorrectly prune edges for a placed member whose event projection
    // was deliberately deferred to its canonical carrier.
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
            "converge_recovered_roster_topology_after_carriers",
        )?;
    }

    // External edges have no second member endpoint through which the member
    // convergence signal can prune them. If the local identity has no durable
    // successor at the end of replay, the respawn command was interrupted and
    // its descriptor-bearing desired edge is abandoned as well.
    let abandoned_external_edges = authority
        .state()
        .external_peer_edges
        .iter()
        .filter(|edge| {
            !authority
                .state()
                .identity_to_runtime
                .contains_key(&edge.local)
        })
        .cloned()
        .collect::<Vec<_>>();
    for edge in abandoned_external_edges {
        apply_seeded_mob_signal(
            authority,
            mob_dsl::MobMachineSignal::RecoverExternalPeerUnwire {
                key: mob_dsl::ExternalPeerKey::new(edge.local.clone(), edge.endpoint.name.clone()),
            },
            "converge_recovered_external_topology_after_carriers",
        )?;
    }

    let abandoned_respawn_holds = authority
        .state()
        .pending_respawn_topology
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    for agent_identity in abandoned_respawn_holds {
        apply_seeded_mob_signal(
            authority,
            mob_dsl::MobMachineSignal::ConvergeRecoveredRespawnTopology { agent_identity },
            "converge_recovered_respawn_topology_after_carriers",
        )?;
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
fn recover_cleanup_only_placed_carrier_signals(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    plan: &PlacedCarrierRecoveryPlan,
) -> Result<(), MobError> {
    validate_placed_carrier_owner_bindings(authority, plan)?;
    for entry in &plan.entries {
        match entry.disposition {
            PlacedCarrierRecoveryDisposition::PendingCleanup => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverPendingPlacedSpawn {
                        spawn_id: mob_dsl::PlacedSpawnId(entry.record.spawn_id.to_string()),
                        agent_identity: mob_dsl::AgentIdentity::from(
                            entry.record.agent_identity.clone(),
                        ),
                        generation: mob_dsl::Generation(entry.record.generation),
                        fence_token: mob_dsl::FenceToken(entry.record.fence_token),
                        host_id: mob_dsl::HostId(entry.record.host_id.to_string()),
                        host_binding_generation: entry.record.host_binding_generation,
                        spec_digest: entry.record.spec_digest.clone(),
                        runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::from(
                            domain_runtime_mode_from_portable(
                                entry.record.spec.profile.runtime_mode,
                            ),
                        ),
                        provision_operation_id: entry.record.provision_operation_id.to_string(),
                        operation_owner_session_id: mob_dsl::SessionId(
                            entry.record.operation_owner_session_id.to_string(),
                        ),
                    },
                    "recover_pending_placed_carrier",
                )?;
            }
            PlacedCarrierRecoveryDisposition::CommittedCleanup => {
                apply_seeded_mob_signal(
                    authority,
                    mob_dsl::MobMachineSignal::RecoverPlacedCarrierCleanup {
                        obligation: placed_carrier_cleanup_obligation(&entry.record),
                    },
                    "recover_committed_placed_carrier_cleanup",
                )?;
            }
            PlacedCarrierRecoveryDisposition::CommittedCurrent => {}
        }
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
fn recover_current_committed_placed_carrier_signals(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    plan: &PlacedCarrierRecoveryPlan,
) -> Result<(), MobError> {
    validate_placed_carrier_owner_bindings(authority, plan)?;
    for entry in &plan.entries {
        if entry.disposition != PlacedCarrierRecoveryDisposition::CommittedCurrent {
            continue;
        }
        let crate::store::PlacedSpawnCarrierPhase::Committed(committed) = &entry.record.phase
        else {
            return Err(MobError::Internal(format!(
                "placed recovery plan classified Pending carrier '{}' as current",
                entry.record.spawn_id
            )));
        };
        let identity = AgentIdentity::from(entry.record.agent_identity.as_str());
        let generation = crate::ids::Generation::new(entry.record.generation);
        apply_seeded_mob_signal(
            authority,
            mob_dsl::MobMachineSignal::RecoverCommittedPlacedSpawn {
                spawn_id: mob_dsl::PlacedSpawnId(entry.record.spawn_id.to_string()),
                agent_identity: mob_dsl::AgentIdentity::from_domain(&identity),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&AgentRuntimeId::new(
                    identity, generation,
                )),
                generation: mob_dsl::Generation::from_domain(generation),
                fence_token: mob_dsl::FenceToken(entry.record.fence_token),
                host_id: mob_dsl::HostId(entry.record.host_id.to_string()),
                host_binding_generation: entry.record.host_binding_generation,
                member_session_id: mob_dsl::SessionId(committed.member_session_id.to_string()),
                member_peer_endpoint: committed.member_peer_endpoint.clone(),
                profile_name: entry.record.spec.profile_name.clone(),
                runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::from(
                    domain_runtime_mode_from_portable(entry.record.spec.profile.runtime_mode),
                ),
                external_addressable: entry.record.spec.profile.external_addressable,
                provision_operation_id: entry.record.provision_operation_id.to_string(),
                operation_owner_session_id: mob_dsl::SessionId(
                    entry.record.operation_owner_session_id.to_string(),
                ),
            },
            "recover_committed_placed_carrier",
        )?;
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
async fn drive_recovered_placed_carrier_cleanup(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    runtime_metadata: &Arc<dyn crate::store::MobRuntimeMetadataStore>,
    mob_id: &MobId,
    plan: &PlacedCarrierRecoveryPlan,
    provisioner: &dyn MobProvisioner,
    supervisor_bridge: Arc<MobSupervisorBridge>,
) -> Result<(), MobError> {
    for entry in &plan.entries {
        if !matches!(
            entry.disposition,
            PlacedCarrierRecoveryDisposition::PendingCleanup
                | PlacedCarrierRecoveryDisposition::CommittedCleanup
        ) {
            continue;
        }
        // Derive the exact compare-delete witness before touching either the
        // host or operation registry.  A corrupt carrier therefore fails at
        // the machine/persistence boundary with zero external side effects.
        // Authorization leaves the cleanup obligation open; only the exact
        // successful CAS below may resolve it.
        let obligation = placed_carrier_cleanup_obligation(&entry.record);
        let authorize = apply_seeded_mob_input_transition(
            authority,
            mob_dsl::MobMachineInput::AuthorizePlacedCarrierCleanup {
                obligation: obligation.clone(),
            },
            "authorize_recovered_placed_carrier_cleanup",
        )?;
        let persistence = crate::store::MobPlacedSpawnCleanupAuthority::from_transition(
            &entry.record,
            &authorize,
        )?;
        // A Pending write is an uncertain MaterializeMember dispatch: its ACK
        // may have been lost after the host committed.  Never delete that
        // durable seed until the shared transport helper has released the
        // exact tuple or authenticated HostStatus has proved it absent.
        if entry.disposition == PlacedCarrierRecoveryDisposition::PendingCleanup {
            super::placed_carrier_cleanup::release_placed_attempt_or_certify_absent(
                mob_id,
                &entry.record,
                authority,
                provisioner,
                Arc::clone(&supervisor_bridge),
            )
            .await?;
            let display_name = super::actor::render_member_comms_name(
                mob_id.as_str(),
                &entry.record.spec.profile_name,
                &entry.record.agent_identity,
            )?;
            provisioner
                .abort_placed_provision_operation(
                    &entry.record.operation_owner_session_id,
                    &entry.record.provision_operation_id,
                    &display_name,
                )
                .await
                .map_err(|error| {
                    MobError::Internal(format!(
                        "cold recovery could not abort exact Pending placed provision operation '{}' owned by session '{}'; carrier retained: {error}",
                        entry.record.provision_operation_id,
                        entry.record.operation_owner_session_id
                    ))
                })?;
        }
        if entry.disposition == PlacedCarrierRecoveryDisposition::CommittedCleanup {
            let display_name = super::actor::render_member_comms_name(
                mob_id.as_str(),
                &entry.record.spec.profile_name,
                &entry.record.agent_identity,
            )?;
            provisioner
                .retire_committed_placed_provision_operation(
                    &entry.record.operation_owner_session_id,
                    &entry.record.provision_operation_id,
                    &display_name,
                )
                .await
                .map_err(|error| {
                    MobError::Internal(format!(
                        "cold recovery could not retire or validate exact committed placed provision operation '{}' owned by session '{}'; carrier retained: {error}",
                        entry.record.provision_operation_id,
                        entry.record.operation_owner_session_id
                    ))
                })?;
        }
        match runtime_metadata
            .compare_and_delete_placed_spawn(mob_id, &entry.record, &persistence)
            .await?
        {
            crate::store::DeletePlacedSpawnResult::Deleted
            | crate::store::DeletePlacedSpawnResult::AlreadyAbsent => {}
            crate::store::DeletePlacedSpawnResult::Conflict => {
                return Err(MobError::Internal(format!(
                    "cold recovery placed-carrier cleanup conflict for identity='{}' spawn_id={} generation={} fence={}; obligation retained",
                    entry.record.agent_identity,
                    entry.record.spawn_id,
                    entry.record.generation,
                    entry.record.fence_token
                )));
            }
        }
        apply_seeded_mob_input(
            authority,
            mob_dsl::MobMachineInput::ResolvePlacedCarrierCleanup { obligation },
            "resolve_recovered_placed_carrier_cleanup",
        )?;
    }
    Ok(())
}

fn recovered_public_phase_from_events(events: &[crate::event::MobEvent]) -> MobState {
    events.iter().fold(MobState::Running, |phase, event| {
        match &event.kind {
            MobEventKind::MobReset => MobState::Running,
            MobEventKind::MobStopped => MobState::Stopped,
            MobEventKind::PlacedCompletionLifecycleQuiesceEnded {
                intent: mob_dsl::PlacedCompletionLifecycleIntentKind::Stop,
            } => MobState::Running,
            MobEventKind::MobCompleted => MobState::Completed,
            // MobDestroying is an admitted cleanup anchor, not terminal
            // machine state. Replay keeps the authority internally Running
            // (with destroy_admitted=true) so placement, custody, and retire
            // cleanup can be rebuilt. seeded_mob_public_phase still projects
            // it as Destroyed to external callers. Only the storage-finalizing
            // fence is terminal for recovery work.
            MobEventKind::MobDestroying => MobState::Running,
            MobEventKind::MobDestroyStorageFinalizing => MobState::Destroyed,
            _ => phase,
        }
    })
}

async fn seed_mob_authority_sync_from_flow_runs(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    run_store: Arc<dyn crate::store::MobRunStore>,
    event_store: Arc<dyn crate::store::MobEventStore>,
    mob_id: &MobId,
    events: &[crate::event::MobEvent],
    deferred_remote_intent_runs: &BTreeSet<RunId>,
) -> Result<(), MobError> {
    use crate::run::MobRun;

    let run_store = authority_validating_mob_run_store(run_store);
    let persisted_terminal_events = events
        .iter()
        .filter(|event| event.mob_id == *mob_id)
        .filter_map(|event| {
            crate::store::terminal_event_identity(&event.kind)
                .map(|(run_id, flow_id)| (run_id.clone(), flow_id.clone()))
        })
        .collect::<BTreeSet<_>>();
    let terminal_event_store = event_store.clone();
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
        let run_terminal =
            crate::run::mob_machine_run_status_is_terminal(&run.run_id, &run.status)?;
        let terminal_key = (run.run_id.clone(), run.flow_id.clone());
        if run_terminal {
            if !persisted_terminal_events.contains(&terminal_key) {
                // The non-durable/test store fallback can commit the terminal
                // run snapshot before its public carrier append fails. Live
                // execution fail-stops at that exact seam; cold recovery
                // repairs only the missing exact terminal carrier. Durable
                // SQLite commits both sides atomically and never enters this
                // branch. Terminal histories retain their established
                // run-store ownership and are not reintroduced into the live
                // MobMachine projection.
                let target = match &run.status {
                    crate::run::MobRunStatus::Completed => {
                        super::terminalization::TerminalizationTarget::Completed {
                            structured_output: None,
                        }
                    }
                    crate::run::MobRunStatus::Failed => {
                        super::terminalization::TerminalizationTarget::Failed {
                            cause: super::terminalization::FlowFailureCause::AlreadyFailed,
                        }
                    }
                    crate::run::MobRunStatus::Canceled => {
                        super::terminalization::TerminalizationTarget::Canceled
                    }
                    crate::run::MobRunStatus::Pending | crate::run::MobRunStatus::Running => {
                        unreachable!("run terminality was checked above")
                    }
                };
                if let Err(repair_error) = terminalization
                    .repair_persisted_terminalization(
                        run.run_id.clone(),
                        run.flow_id.clone(),
                        target,
                    )
                    .await
                {
                    // A backend may commit the idempotent carrier and lose its
                    // acknowledgement. Accept that distributed outcome only
                    // after replay proves this exact mob/run/flow carrier;
                    // otherwise keep recovery failed closed and retryable.
                    let exact_repair_is_durable = terminal_event_store
                        .replay_all()
                        .await?
                        .iter()
                        .any(|event| {
                            event.mob_id == *mob_id
                                && crate::store::terminal_event_identity(&event.kind).is_some_and(
                                    |(run_id, flow_id)| {
                                        run_id == &terminal_key.0 && flow_id == &terminal_key.1
                                    },
                                )
                        });
                    if !exact_repair_is_durable {
                        return Err(repair_error);
                    }
                }
            }
            continue;
        }
        // Scheduler/frame cross-projection checks are recovery preconditions
        // for ACTIVE runs only. Terminalization releases run-level scheduler
        // slots but deliberately does not rewrite an interrupted frame's
        // historical node state; applying active-run reconciliation to that
        // terminal history therefore manufactures a false
        // `active_node_count` mismatch. The authority-validating run-store
        // wrapper above has already replayed and verified every terminal
        // aggregate against its exact persisted MobMachine input log.
        super::recovery::reconcile_run_state(&mut run).map_err(|error| {
            MobError::Internal(format!("cannot resume flow run '{}': {error}", run.run_id))
        })?;
        if run.flow_authority_inputs.is_empty() {
            continue;
        }

        MobRun::replay_flow_authority_inputs_into(
            authority,
            &run.flow_authority_inputs,
            &format!("resume_flow_run_{}", run.run_id),
        )?;
        if flow_run_replayed_active_admission(&run)
            && !deferred_remote_intent_runs.contains(&run.run_id)
        {
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

fn remote_turn_obligation_from_event(
    obligation: &crate::event::RemoteTurnObligationEvent,
) -> mob_dsl::RemoteTurnObligation {
    mob_dsl::RemoteTurnObligation {
        agent_identity: mob_dsl::AgentIdentity(obligation.agent_identity.to_string()),
        host_id: mob_dsl::HostId(obligation.host_id.clone()),
        host_binding_generation: obligation.host_binding_generation,
        member_session_id: mob_dsl::SessionId(obligation.member_session_id.clone()),
        generation: mob_dsl::Generation(obligation.generation.get()),
        fence_token: mob_dsl::FenceToken(obligation.fence_token.get()),
        dispatch_sequence: obligation.dispatch_sequence,
        input_id: mob_dsl::InputId(obligation.input_id.clone()),
        run_id: mob_dsl::RunId::from(obligation.run_id.to_string()),
        step_id: mob_dsl::StepId::from(obligation.step_id.to_string()),
    }
}

fn recover_remote_turn_outcome_custody(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    events: &[crate::event::MobEvent],
    intents: &[crate::run::MobRunRemoteTurnIntent],
    receipts: &[crate::run::MobRunRemoteTurnReceipt],
) -> Result<(), MobError> {
    #[derive(Clone)]
    enum RecoveredTerminal {
        Completed(serde_json::Value),
        Failed(String),
    }

    #[derive(Default)]
    struct Recovery {
        obligation: Option<mob_dsl::RemoteTurnObligation>,
        recorded: bool,
        has_intent: bool,
        has_receipt: bool,
        committed: bool,
        resolved: bool,
        acknowledged: bool,
        disposed: bool,
        terminal: Option<RecoveredTerminal>,
    }

    let mut recoveries = BTreeMap::<u64, Recovery>::new();
    for intent in intents {
        let obligation = super::remote_flow_ticket::obligation_from_event(&intent.obligation);
        let recovery = recoveries.entry(obligation.dispatch_sequence).or_default();
        if recovery
            .obligation
            .as_ref()
            .is_some_and(|existing| existing != &obligation)
        {
            return Err(MobError::Internal(format!(
                "remote-turn intent sequence {} conflicts with another exact obligation",
                obligation.dispatch_sequence
            )));
        }
        recovery.obligation = Some(obligation);
        // The replay-complete private intent is authority when the public
        // Record append was ambiguous. Recovery repairs that projection and
        // must still reserve/record the exact sequence before any resend.
        recovery.recorded = true;
        recovery.has_intent = true;
    }
    for event in events {
        let (event_obligation, phase, terminal) = match &event.kind {
            MobEventKind::RemoteTurnObligationRecorded { obligation } => (obligation, 0u8, None),
            MobEventKind::StepTargetCompleted {
                run_id,
                step_id,
                target,
                remote_turn_obligation: Some(obligation),
                output,
            } => {
                if run_id != &obligation.run_id
                    || step_id != &obligation.step_id
                    || target.identity != obligation.agent_identity
                    || target.generation != obligation.generation
                {
                    return Err(MobError::Internal(format!(
                        "remote-turn terminal carrier at event cursor {} does not match its outer run/step/target fields",
                        event.cursor
                    )));
                }
                let output = output.clone().ok_or_else(|| {
                    MobError::Internal(format!(
                        "remote-turn completed carrier at event cursor {} has no replay output",
                        event.cursor
                    ))
                })?;
                (obligation, 1, Some(RecoveredTerminal::Completed(output)))
            }
            MobEventKind::StepTargetFailed {
                run_id,
                step_id,
                target,
                remote_turn_obligation: Some(obligation),
                reason,
                ..
            } => {
                if run_id != &obligation.run_id
                    || step_id != &obligation.step_id
                    || target.identity != obligation.agent_identity
                    || target.generation != obligation.generation
                {
                    return Err(MobError::Internal(format!(
                        "remote-turn terminal carrier at event cursor {} does not match its outer run/step/target fields",
                        event.cursor
                    )));
                }
                (
                    obligation,
                    1,
                    Some(RecoveredTerminal::Failed(reason.clone())),
                )
            }
            MobEventKind::RemoteTurnOutcomeResolved { obligation } => (obligation, 2, None),
            MobEventKind::RemoteTurnOutcomeAcknowledged { obligation } => (obligation, 3, None),
            MobEventKind::RemoteTurnOutcomeDisposed { obligation } => (obligation, 4, None),
            _ => continue,
        };
        let obligation = remote_turn_obligation_from_event(event_obligation);
        let recovery = recoveries.entry(obligation.dispatch_sequence).or_default();
        if recovery
            .obligation
            .as_ref()
            .is_some_and(|existing| existing != &obligation)
        {
            return Err(MobError::Internal(format!(
                "remote-turn dispatch sequence {} names conflicting obligations during recovery",
                obligation.dispatch_sequence
            )));
        }
        recovery.obligation = Some(obligation);
        if let Some(terminal) = terminal {
            if recovery.terminal.is_some() {
                return Err(MobError::Internal(format!(
                    "remote-turn dispatch sequence {} has duplicate terminal carriers",
                    event_obligation.dispatch_sequence
                )));
            }
            recovery.terminal = Some(terminal);
        }
        match phase {
            0 => recovery.recorded = true,
            1 => recovery.committed = true,
            2 => recovery.resolved = true,
            3 => recovery.acknowledged = true,
            4 => recovery.disposed = true,
            other => {
                return Err(MobError::Internal(format!(
                    "remote-turn recovery decoded unsupported custody phase {other}"
                )));
            }
        }
    }

    for receipt in receipts {
        let obligation = super::remote_flow_ticket::obligation_from_event(&receipt.obligation);
        let recovery = recoveries.entry(obligation.dispatch_sequence).or_default();
        if recovery
            .obligation
            .as_ref()
            .is_some_and(|existing| existing != &obligation)
        {
            return Err(MobError::Internal(format!(
                "remote-turn receipt sequence {} conflicts with its durable intent",
                obligation.dispatch_sequence
            )));
        }
        recovery.obligation = Some(obligation);
        recovery.has_receipt = true;
        if let Some(terminal) = &recovery.terminal {
            let outcome_matches = match (terminal, &receipt.outcome) {
                (
                    RecoveredTerminal::Completed(event_value),
                    crate::run::MobRunRemoteTurnReceiptOutcome::Completed { value },
                ) => event_value == value,
                (
                    RecoveredTerminal::Failed(event_reason),
                    crate::run::MobRunRemoteTurnReceiptOutcome::Failed { reason, .. }
                    | crate::run::MobRunRemoteTurnReceiptOutcome::TrackedInputCancel {
                        reason, ..
                    },
                ) => event_reason == reason,
                _ => false,
            };
            if !outcome_matches {
                return Err(MobError::Internal(format!(
                    "remote-turn receipt sequence {} does not match its exact terminal carrier",
                    receipt.obligation.dispatch_sequence
                )));
            }
        }
    }

    // Durable carrier appends from concurrent flow tasks can be physically
    // reordered. Reconstruct by the machine-owned dispatch sequence, then by
    // monotonic custody phase, never by incidental event cursor order.
    for (dispatch_sequence, recovery) in recoveries {
        let obligation = recovery.obligation.ok_or_else(|| {
            MobError::Internal(format!(
                "remote-turn dispatch sequence {dispatch_sequence} has no recoverable obligation"
            ))
        })?;
        if !recovery.recorded {
            return Err(MobError::Internal(format!(
                "remote-turn dispatch sequence {dispatch_sequence} has phase carriers but no durable Record carrier"
            )));
        }
        if recovery.acknowledged && recovery.disposed {
            return Err(MobError::Internal(format!(
                "remote-turn dispatch sequence {dispatch_sequence} has mutually exclusive ACK and Dispose finalizers"
            )));
        }
        if !recovery.disposed
            && ((recovery.resolved && !recovery.committed)
                || (recovery.acknowledged && !recovery.resolved))
        {
            return Err(MobError::Internal(format!(
                "remote-turn dispatch sequence {dispatch_sequence} has a custody phase without its durable predecessor"
            )));
        }
        if recovery.disposed && !recovery.committed {
            return Err(MobError::Internal(format!(
                "remote-turn dispatch sequence {dispatch_sequence} was disposed without its terminal carrier"
            )));
        }
        // ACK/Dispose is the privacy-cleanup boundary: startup intentionally
        // deletes the private intent and receipt once either public finalizer
        // is durable. Validate the remaining public chain above, then recover
        // only the burned sequence. Requiring a receipt before this branch
        // would reject every normally finalized remote turn after restart.
        if recovery.disposed || recovery.acknowledged {
            apply_seeded_mob_signal(
                authority,
                mob_dsl::MobMachineSignal::RecoverRemoteTurnDispatchSequence { dispatch_sequence },
                "recover_finalized_remote_turn_dispatch_sequence",
            )?;
            continue;
        }
        if recovery.committed && !recovery.has_receipt {
            return Err(MobError::Internal(format!(
                "remote-turn dispatch sequence {dispatch_sequence} has a terminal carrier without its durable receipt"
            )));
        }
        if !recovery.has_intent {
            return Err(MobError::Internal(format!(
                "nonfinal remote-turn dispatch sequence {dispatch_sequence} has no replay-complete private intent"
            )));
        }
        apply_seeded_mob_input(
            authority,
            mob_dsl::MobMachineInput::RecordRemoteTurnObligation {
                obligation: obligation.clone(),
            },
            "recover_remote_turn_record",
        )?;
        if recovery.committed {
            apply_seeded_mob_input(
                authority,
                mob_dsl::MobMachineInput::CommitRemoteTurnOutcome {
                    obligation: obligation.clone(),
                },
                "recover_remote_turn_committed",
            )?;
        }
        if recovery.resolved {
            apply_seeded_mob_input(
                authority,
                mob_dsl::MobMachineInput::ResolveRemoteTurnObligation {
                    obligation: obligation.clone(),
                },
                "recover_remote_turn_resolved",
            )?;
        }
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Default, Clone)]
struct PlacedCompletionRecoveryChain {
    obligation: Option<crate::event::PlacedCompletionObligationEvent>,
    recorded: bool,
    cancellation_requested: bool,
    resolved: Option<crate::event::PlacedCompletionHostOutcomeEvent>,
    closed: Option<crate::event::PlacedCompletionClosureEvent>,
    acknowledged: bool,
    disposed: bool,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PlacedCompletionTurnKey {
    agent_identity: String,
    host_id: String,
    generation: u64,
    fence_token: u64,
    input_id: String,
}

#[cfg(feature = "runtime-adapter")]
impl From<&crate::event::PlacedCompletionObligationEvent> for PlacedCompletionTurnKey {
    fn from(obligation: &crate::event::PlacedCompletionObligationEvent) -> Self {
        Self {
            agent_identity: obligation.agent_identity.to_string(),
            host_id: obligation.host_id.clone(),
            generation: obligation.generation.get(),
            fence_token: obligation.fence_token.get(),
            input_id: obligation.input_id.clone(),
        }
    }
}

#[cfg(feature = "runtime-adapter")]
fn collect_placed_completion_recovery_chains(
    events: &[crate::event::MobEvent],
) -> Result<BTreeMap<u64, PlacedCompletionRecoveryChain>, MobError> {
    let mut chains = BTreeMap::<u64, PlacedCompletionRecoveryChain>::new();
    let mut by_input = BTreeMap::<PlacedCompletionTurnKey, u64>::new();
    for event in events {
        let (obligation, phase) = match &event.kind {
            MobEventKind::PlacedCompletionObligationRecorded { obligation } => (obligation, 0u8),
            MobEventKind::PlacedCompletionCancellationRequested { obligation } => (obligation, 1),
            MobEventKind::PlacedCompletionOutcomeResolved { obligation, .. } => (obligation, 2),
            MobEventKind::PlacedCompletionOutcomeClosed { obligation, .. } => (obligation, 3),
            MobEventKind::PlacedCompletionOutcomeAcknowledged { obligation } => (obligation, 4),
            MobEventKind::PlacedCompletionOutcomeDisposed { obligation } => (obligation, 5),
            _ => continue,
        };
        if let Some(existing) = by_input.insert(
            PlacedCompletionTurnKey::from(obligation),
            obligation.dispatch_sequence,
        ) && existing != obligation.dispatch_sequence
        {
            return Err(MobError::Internal(format!(
                "placed completion input '{}' names multiple dispatch sequences",
                obligation.input_id
            )));
        }
        let chain = chains.entry(obligation.dispatch_sequence).or_default();
        if chain
            .obligation
            .as_ref()
            .is_some_and(|existing| existing != obligation)
        {
            return Err(MobError::Internal(format!(
                "placed completion sequence {} names conflicting obligations",
                obligation.dispatch_sequence
            )));
        }
        chain.obligation = Some(obligation.clone());
        match phase {
            0 => {
                if chain.recorded {
                    return Err(MobError::Internal(format!(
                        "placed completion sequence {} has duplicate Record carriers",
                        obligation.dispatch_sequence
                    )));
                }
                chain.recorded = true;
            }
            1 => {
                if !chain.recorded
                    || chain.cancellation_requested
                    || chain.resolved.is_some()
                    || chain.closed.is_some()
                    || chain.acknowledged
                    || chain.disposed
                {
                    return Err(MobError::Internal(format!(
                        "placed completion sequence {} has an out-of-order cancellation carrier",
                        obligation.dispatch_sequence
                    )));
                }
                chain.cancellation_requested = true;
            }
            2 => {
                let MobEventKind::PlacedCompletionOutcomeResolved { outcome, .. } = &event.kind
                else {
                    return Err(MobError::Internal(
                        "placed completion recovery phase lost its Resolve carrier".to_string(),
                    ));
                };
                if !chain.recorded
                    || chain.resolved.is_some()
                    || chain.closed.is_some()
                    || chain.acknowledged
                    || chain.disposed
                {
                    return Err(MobError::Internal(format!(
                        "placed completion sequence {} has an out-of-order/conflicting Resolve",
                        obligation.dispatch_sequence
                    )));
                }
                chain.resolved = Some(outcome.clone());
            }
            3 => {
                let MobEventKind::PlacedCompletionOutcomeClosed { closure, .. } = &event.kind
                else {
                    return Err(MobError::Internal(
                        "placed completion recovery phase lost its Close carrier".to_string(),
                    ));
                };
                if !chain.recorded
                    || !chain.cancellation_requested
                    || chain.resolved.is_some()
                    || chain.closed.is_some()
                    || chain.acknowledged
                    || chain.disposed
                {
                    return Err(MobError::Internal(format!(
                        "placed completion sequence {} has an out-of-order/conflicting Close without its cancellation predecessor",
                        obligation.dispatch_sequence
                    )));
                }
                chain.closed = Some(*closure);
            }
            4 => {
                if chain.resolved.is_none() || chain.acknowledged || chain.disposed {
                    return Err(MobError::Internal(format!(
                        "placed completion sequence {} ACK has no unique Resolve predecessor",
                        obligation.dispatch_sequence
                    )));
                }
                chain.acknowledged = true;
            }
            5 => {
                if !chain.recorded || chain.disposed || chain.acknowledged || chain.closed.is_some()
                {
                    return Err(MobError::Internal(format!(
                        "placed completion sequence {} has an out-of-order Dispose",
                        obligation.dispatch_sequence
                    )));
                }
                chain.disposed = true;
            }
            _ => {
                return Err(MobError::Internal(
                    "placed completion recovery produced an unknown custody phase".to_string(),
                ));
            }
        }
    }
    for (sequence, chain) in &chains {
        if !chain.recorded {
            return Err(MobError::Internal(format!(
                "placed completion sequence {sequence} has phase carriers without Record"
            )));
        }
    }
    Ok(chains)
}

/// Rebuild ordinary completion custody before any pump/reconciler exists.
/// Every nonfinal Pending row is durably switched to exact cancellation: an
/// RPC promise cannot survive restart, so recovery never re-originates work.
#[cfg(feature = "runtime-adapter")]
async fn recover_placed_completion_outcome_custody(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    event_store: &Arc<dyn crate::store::MobEventStore>,
    mob_id: &MobId,
    events: &mut Vec<crate::event::MobEvent>,
) -> Result<Option<mob_dsl::PlacedCompletionLifecycleIntentKind>, MobError> {
    let epoch_start = events
        .iter()
        .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
        .map_or(0, |position| position + 1);
    let mut chains = collect_placed_completion_recovery_chains(&events[epoch_start..])?;
    let missing_cancellations = chains
        .values()
        .filter(|chain| {
            !chain.cancellation_requested
                && chain.resolved.is_none()
                && chain.closed.is_none()
                && !chain.acknowledged
                && !chain.disposed
        })
        .map(|chain| {
            chain
                .obligation
                .clone()
                .map(
                    |obligation| MobEventKind::PlacedCompletionCancellationRequested { obligation },
                )
                .ok_or_else(|| {
                    MobError::Internal(
                        "placed completion cancellation candidate has no Record carrier"
                            .to_string(),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !missing_cancellations.is_empty() {
        let batch = missing_cancellations
            .iter()
            .cloned()
            .map(|kind| NewMobEvent {
                mob_id: mob_id.clone(),
                timestamp: None,
                kind,
            })
            .collect::<Vec<_>>();
        match event_store.append_batch(batch).await {
            Ok(appended) => events.extend(appended),
            Err(error) => {
                let replay = event_store
                    .replay_all()
                    .await?
                    .into_iter()
                    .filter(|event| &event.mob_id == mob_id)
                    .collect::<Vec<_>>();
                if missing_cancellations
                    .iter()
                    .any(|desired| !replay.iter().any(|event| event.kind == *desired))
                {
                    return Err(MobError::Internal(format!(
                        "placed completion cold-cancellation repair failed without exact durable convergence: {error}"
                    )));
                }
                *events = replay;
            }
        }
        let epoch_start = events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |position| position + 1);
        chains = collect_placed_completion_recovery_chains(&events[epoch_start..])?;
    }

    for (dispatch_sequence, chain) in chains {
        let event = chain.obligation.ok_or_else(|| {
            MobError::Internal(format!(
                "placed completion sequence {dispatch_sequence} has no obligation"
            ))
        })?;
        let obligation = super::placed_completion_reconciler::obligation_from_event(&event)?;
        if chain.closed.is_some() || chain.acknowledged || chain.disposed {
            apply_seeded_mob_signal(
                authority,
                mob_dsl::MobMachineSignal::RecoverPlacedCompletionDispatchSequence {
                    dispatch_sequence,
                },
                "recover_finalized_placed_completion_sequence",
            )?;
            continue;
        }
        if chain.resolved.is_some() {
            apply_seeded_mob_signal(
                authority,
                mob_dsl::MobMachineSignal::RecoverResolvedPlacedCompletion { obligation },
                "recover_placed_completion_resolved",
            )?;
        } else {
            apply_seeded_mob_signal(
                authority,
                mob_dsl::MobMachineSignal::RecoverPendingPlacedCompletion {
                    obligation,
                    cancellation_requested: true,
                },
                "recover_placed_completion_cancellation",
            )?;
        }
    }
    let mut quiescing = None;
    for event in events.iter() {
        match event.kind {
            MobEventKind::PlacedCompletionLifecycleQuiesceStarted { intent } => {
                quiescing = Some(intent);
            }
            MobEventKind::PlacedCompletionLifecycleQuiesceEnded { .. } | MobEventKind::MobReset => {
                quiescing = None;
            }
            _ => {}
        }
    }
    Ok(quiescing)
}

#[cfg(feature = "runtime-adapter")]
fn apply_recovered_placed_completion_lifecycle_intent(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    intent: Option<mob_dsl::PlacedCompletionLifecycleIntentKind>,
) -> Result<(), MobError> {
    if let Some(intent) = intent
        && authority.state().placed_completion_lifecycle_intent != Some(intent)
    {
        apply_seeded_mob_input(
            authority,
            mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce { intent },
            "recover_placed_completion_quiesce",
        )?;
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PlacedKickoffRecoveryKey {
    agent_identity: String,
    host_id: String,
    host_binding_generation: u64,
    member_session_id: String,
    generation: u64,
    fence_token: u64,
    input_id: String,
    objective_id: String,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PlacedKickoffTurnKey {
    agent_identity: String,
    host_id: String,
    generation: u64,
    fence_token: u64,
    input_id: String,
}

#[cfg(feature = "runtime-adapter")]
impl From<&crate::event::PlacedKickoffObligationEvent> for PlacedKickoffTurnKey {
    fn from(obligation: &crate::event::PlacedKickoffObligationEvent) -> Self {
        Self {
            agent_identity: obligation.agent_identity.to_string(),
            host_id: obligation.host_id.clone(),
            generation: obligation.generation.get(),
            fence_token: obligation.fence_token.get(),
            input_id: obligation.input_id.clone(),
        }
    }
}

#[cfg(feature = "runtime-adapter")]
impl From<&PlacedKickoffRecoveryKey> for PlacedKickoffTurnKey {
    fn from(obligation: &PlacedKickoffRecoveryKey) -> Self {
        Self {
            agent_identity: obligation.agent_identity.clone(),
            host_id: obligation.host_id.clone(),
            generation: obligation.generation,
            fence_token: obligation.fence_token,
            input_id: obligation.input_id.clone(),
        }
    }
}

#[cfg(feature = "runtime-adapter")]
impl PlacedKickoffRecoveryKey {
    fn from_event(obligation: &crate::event::PlacedKickoffObligationEvent) -> Self {
        Self {
            agent_identity: obligation.agent_identity.to_string(),
            host_id: obligation.host_id.clone(),
            host_binding_generation: obligation.host_binding_generation,
            member_session_id: obligation.member_session_id.clone(),
            generation: obligation.generation.get(),
            fence_token: obligation.fence_token.get(),
            input_id: obligation.input_id.clone(),
            objective_id: obligation.objective_id.to_string(),
        }
    }
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone)]
struct RecoveredPlacedKickoffResolution {
    outcome: crate::event::PlacedKickoffHostOutcomeEvent,
    kickoff: crate::roster::MobMemberKickoffSnapshot,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone)]
struct RecoveredPlacedKickoffRejection {
    error: String,
    kickoff: crate::roster::MobMemberKickoffSnapshot,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Default)]
struct PlacedKickoffRecoveryChain {
    obligation: Option<crate::event::PlacedKickoffObligationEvent>,
    recorded: bool,
    recorded_cursor: Option<u64>,
    resolved: Option<RecoveredPlacedKickoffResolution>,
    rejected_no_effect: Option<RecoveredPlacedKickoffRejection>,
    acknowledged: bool,
    disposed: Option<crate::roster::MobMemberKickoffSnapshot>,
}

#[cfg(feature = "runtime-adapter")]
fn paired_kickoff_snapshot<'a>(
    events: &'a [crate::event::MobEvent],
    event_index: usize,
    identity: &AgentIdentity,
    expected: Option<&crate::roster::MobMemberKickoffSnapshot>,
    carrier: &str,
) -> Result<&'a crate::roster::MobMemberKickoffSnapshot, MobError> {
    let Some(next) = events.get(event_index + 1) else {
        return Err(MobError::Internal(format!(
            "{carrier} at the end of the mob event log has no atomic kickoff projection"
        )));
    };
    let MobEventKind::MemberKickoffUpdated { member, kickoff } = &next.kind else {
        return Err(MobError::Internal(format!(
            "{carrier} is not immediately followed by its atomic kickoff projection"
        )));
    };
    if member != identity || expected.is_some_and(|expected| expected != kickoff) {
        return Err(MobError::Internal(format!(
            "{carrier} kickoff projection does not exactly match its custody carrier"
        )));
    }
    Ok(kickoff)
}

#[cfg(feature = "runtime-adapter")]
fn collect_placed_kickoff_recovery_chains(
    events: &[crate::event::MobEvent],
) -> Result<BTreeMap<PlacedKickoffRecoveryKey, PlacedKickoffRecoveryChain>, MobError> {
    let mut chains = BTreeMap::<PlacedKickoffRecoveryKey, PlacedKickoffRecoveryChain>::new();
    let mut by_input = BTreeMap::<PlacedKickoffTurnKey, PlacedKickoffRecoveryKey>::new();

    for (event_index, event) in events.iter().enumerate() {
        let (obligation, phase) = match &event.kind {
            MobEventKind::PlacedKickoffObligationRecorded { obligation } => (obligation, 0_u8),
            MobEventKind::PlacedKickoffOutcomeResolved { obligation, .. } => (obligation, 1_u8),
            MobEventKind::PlacedKickoffRejectedNoEffect { obligation, .. } => (obligation, 2_u8),
            MobEventKind::PlacedKickoffOutcomeAcknowledged { obligation } => (obligation, 3_u8),
            MobEventKind::PlacedKickoffOutcomeDisposed { obligation } => (obligation, 4_u8),
            _ => continue,
        };
        let key = PlacedKickoffRecoveryKey::from_event(obligation);
        let turn_key = PlacedKickoffTurnKey::from(obligation);
        if let Some(existing) = by_input.get(&turn_key)
            && existing != &key
        {
            return Err(MobError::Internal(format!(
                "placed kickoff input id '{}' is reused by conflicting custody tuples",
                obligation.input_id
            )));
        }
        by_input.insert(turn_key, key.clone());
        let chain = chains.entry(key).or_default();
        if chain
            .obligation
            .as_ref()
            .is_some_and(|existing| existing != obligation)
        {
            return Err(MobError::Internal(format!(
                "placed kickoff input id '{}' names conflicting obligations",
                obligation.input_id
            )));
        }
        chain.obligation = Some(obligation.clone());
        match phase {
            0 => {
                if chain.recorded {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' has duplicate Record carriers",
                        obligation.input_id
                    )));
                }
                let kickoff = paired_kickoff_snapshot(
                    events,
                    event_index,
                    &obligation.agent_identity,
                    None,
                    "PlacedKickoffObligationRecorded",
                )?;
                if kickoff.objective_id != Some(obligation.objective_id)
                    || kickoff.phase != crate::roster::MobMemberKickoffPhase::Starting
                    || kickoff.error.is_some()
                {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' Record has a non-Starting or mismatched kickoff projection",
                        obligation.input_id
                    )));
                }
                chain.recorded = true;
                chain.recorded_cursor = Some(event.cursor);
            }
            1 => {
                let MobEventKind::PlacedKickoffOutcomeResolved {
                    outcome, kickoff, ..
                } = &event.kind
                else {
                    return Err(MobError::Internal(
                        "placed kickoff recovery phase lost its Resolve carrier".to_string(),
                    ));
                };
                if chain.resolved.is_some() || chain.rejected_no_effect.is_some() {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' has duplicate/conflicting terminal carriers",
                        obligation.input_id
                    )));
                }
                if !chain.recorded || chain.acknowledged || chain.disposed.is_some() {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' resolved outside its recorded Pending custody",
                        obligation.input_id
                    )));
                }
                paired_kickoff_snapshot(
                    events,
                    event_index,
                    &obligation.agent_identity,
                    Some(kickoff),
                    "PlacedKickoffOutcomeResolved",
                )?;
                validate_recovered_kickoff_resolution(obligation, outcome, kickoff)?;
                chain.resolved = Some(RecoveredPlacedKickoffResolution {
                    outcome: outcome.clone(),
                    kickoff: kickoff.clone(),
                });
            }
            2 => {
                let MobEventKind::PlacedKickoffRejectedNoEffect { error, kickoff, .. } =
                    &event.kind
                else {
                    return Err(MobError::Internal(
                        "placed kickoff recovery phase lost its no-effect rejection carrier"
                            .to_string(),
                    ));
                };
                if chain.resolved.is_some() || chain.rejected_no_effect.is_some() {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' has duplicate/conflicting terminal carriers",
                        obligation.input_id
                    )));
                }
                if !chain.recorded || chain.acknowledged || chain.disposed.is_some() {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' rejection is outside its recorded Pending custody",
                        obligation.input_id
                    )));
                }
                paired_kickoff_snapshot(
                    events,
                    event_index,
                    &obligation.agent_identity,
                    Some(kickoff),
                    "PlacedKickoffRejectedNoEffect",
                )?;
                validate_recovered_kickoff_rejection(obligation, error, kickoff)?;
                chain.rejected_no_effect = Some(RecoveredPlacedKickoffRejection {
                    error: error.clone(),
                    kickoff: kickoff.clone(),
                });
            }
            3 => {
                if chain.acknowledged {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' has duplicate ACK carriers",
                        obligation.input_id
                    )));
                }
                if !chain.recorded
                    || chain.resolved.is_none()
                    || chain.rejected_no_effect.is_some()
                    || chain.disposed.is_some()
                {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' ACK is outside Resolved custody",
                        obligation.input_id
                    )));
                }
                chain.acknowledged = true;
            }
            4 => {
                if chain.disposed.is_some() {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' has duplicate Dispose carriers",
                        obligation.input_id
                    )));
                }
                if !chain.recorded || chain.acknowledged || chain.rejected_no_effect.is_some() {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' Dispose is outside active host custody",
                        obligation.input_id
                    )));
                }
                let kickoff = paired_kickoff_snapshot(
                    events,
                    event_index,
                    &obligation.agent_identity,
                    None,
                    "PlacedKickoffOutcomeDisposed",
                )?;
                validate_recovered_kickoff_snapshot(obligation, kickoff)?;
                if let Some(resolved) = chain.resolved.as_ref()
                    && (resolved.kickoff.objective_id != kickoff.objective_id
                        || resolved.kickoff.phase != kickoff.phase
                        || resolved.kickoff.error != kickoff.error)
                {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' Dispose changes its resolved terminal",
                        obligation.input_id
                    )));
                }
                if chain.resolved.is_none()
                    && kickoff.phase != crate::roster::MobMemberKickoffPhase::Cancelled
                {
                    return Err(MobError::Internal(format!(
                        "placed kickoff '{}' pending Dispose is not Cancelled",
                        obligation.input_id
                    )));
                }
                chain.disposed = Some(kickoff.clone());
            }
            _ => {
                return Err(MobError::Internal(
                    "placed kickoff recovery produced an unknown custody phase".to_string(),
                ));
            }
        }
    }

    for chain in chains.values() {
        let obligation = chain.obligation.as_ref().ok_or_else(|| {
            MobError::Internal("placed kickoff recovery chain has no obligation".to_string())
        })?;
        if !chain.recorded {
            return Err(MobError::Internal(format!(
                "placed kickoff '{}' has a custody phase without its Record carrier",
                obligation.input_id
            )));
        }
        if chain.acknowledged && chain.resolved.is_none() {
            return Err(MobError::Internal(format!(
                "placed kickoff '{}' has an ACK without a resolved host terminal",
                obligation.input_id
            )));
        }
        if chain.acknowledged && chain.disposed.is_some() {
            return Err(MobError::Internal(format!(
                "placed kickoff '{}' is both ACKed and disposed",
                obligation.input_id
            )));
        }
        if chain.rejected_no_effect.is_some()
            && (chain.resolved.is_some() || chain.acknowledged || chain.disposed.is_some())
        {
            return Err(MobError::Internal(format!(
                "placed kickoff '{}' has a no-effect rejection mixed with host custody",
                obligation.input_id
            )));
        }
    }
    Ok(chains)
}

#[cfg(feature = "runtime-adapter")]
fn current_placed_spawn_event_cursor(
    events: &[crate::event::MobEvent],
    carrier: &crate::store::MobPlacedSpawnCarrierRecord,
) -> Result<u64, MobError> {
    events
        .iter()
        .find_map(|event| match &event.kind {
            MobEventKind::MemberSpawned(spawned)
                if spawned.placed_spawn_id() == Some(&carrier.spawn_id)
                    && spawned.agent_identity.as_str() == carrier.agent_identity
                    && spawned.generation.get() == carrier.generation =>
            {
                Some(event.cursor)
            }
            _ => None,
        })
        .ok_or_else(|| {
            MobError::Internal(format!(
                "current placed carrier '{}' has no exact MemberSpawned recovery cursor",
                carrier.spawn_id
            ))
        })
}

#[cfg(feature = "runtime-adapter")]
fn latest_kickoff_after_spawn<'a>(
    events: &'a [crate::event::MobEvent],
    spawn_cursor: u64,
    identity: &AgentIdentity,
) -> Option<&'a crate::roster::MobMemberKickoffSnapshot> {
    events.iter().rev().find_map(|event| {
        if event.cursor <= spawn_cursor {
            return None;
        }
        match &event.kind {
            MobEventKind::MemberKickoffUpdated { member, kickoff } if member == identity => {
                Some(kickoff)
            }
            _ => None,
        }
    })
}

#[cfg(feature = "runtime-adapter")]
fn validate_recovered_kickoff_snapshot(
    obligation: &crate::event::PlacedKickoffObligationEvent,
    kickoff: &crate::roster::MobMemberKickoffSnapshot,
) -> Result<(), MobError> {
    if kickoff.objective_id != Some(obligation.objective_id) {
        return Err(MobError::Internal(format!(
            "placed kickoff '{}' terminal objective does not match custody",
            obligation.input_id
        )));
    }
    match kickoff.phase {
        crate::roster::MobMemberKickoffPhase::Started
        | crate::roster::MobMemberKickoffPhase::CallbackPending
        | crate::roster::MobMemberKickoffPhase::Cancelled => {
            if kickoff.error.is_some() {
                return Err(MobError::Internal(format!(
                    "placed kickoff '{}' non-failure terminal carries an error",
                    obligation.input_id
                )));
            }
        }
        crate::roster::MobMemberKickoffPhase::Failed => {
            if kickoff.error.is_none() {
                return Err(MobError::Internal(format!(
                    "placed kickoff '{}' failure terminal has no error",
                    obligation.input_id
                )));
            }
        }
        crate::roster::MobMemberKickoffPhase::Pending
        | crate::roster::MobMemberKickoffPhase::Starting => {
            return Err(MobError::Internal(format!(
                "placed kickoff '{}' outcome carrier is not terminal",
                obligation.input_id
            )));
        }
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
fn validate_recovered_kickoff_resolution(
    obligation: &crate::event::PlacedKickoffObligationEvent,
    outcome: &crate::event::PlacedKickoffHostOutcomeEvent,
    kickoff: &crate::roster::MobMemberKickoffSnapshot,
) -> Result<(), MobError> {
    validate_recovered_kickoff_snapshot(obligation, kickoff)?;
    use crate::event::PlacedKickoffHostOutcomeEvent;
    use crate::roster::MobMemberKickoffPhase;

    // Stop owns the public lifecycle race, but not the host-terminal fact.
    // Any exact host outcome can therefore pair with Cancelled; recovery
    // replays cancellation first and the outcome through its generated
    // AfterCancellation arm.
    if kickoff.phase == MobMemberKickoffPhase::Cancelled {
        return Ok(());
    }
    let exact = match outcome {
        PlacedKickoffHostOutcomeEvent::InteractionComplete => {
            kickoff.phase == MobMemberKickoffPhase::Started && kickoff.error.is_none()
        }
        PlacedKickoffHostOutcomeEvent::InteractionCallbackPending => {
            kickoff.phase == MobMemberKickoffPhase::CallbackPending && kickoff.error.is_none()
        }
        PlacedKickoffHostOutcomeEvent::InteractionFailed { error } => {
            kickoff.phase == MobMemberKickoffPhase::Failed
                && kickoff.error.as_deref() == Some(error.as_str())
        }
        PlacedKickoffHostOutcomeEvent::InteractionCancelled => {
            kickoff.phase == MobMemberKickoffPhase::Cancelled && kickoff.error.is_none()
        }
    };
    if !exact {
        return Err(MobError::Internal(format!(
            "placed kickoff '{}' host outcome does not match its lifecycle projection",
            obligation.input_id
        )));
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
fn validate_recovered_kickoff_rejection(
    obligation: &crate::event::PlacedKickoffObligationEvent,
    error: &str,
    kickoff: &crate::roster::MobMemberKickoffSnapshot,
) -> Result<(), MobError> {
    validate_recovered_kickoff_snapshot(obligation, kickoff)?;
    if error.is_empty() {
        return Err(MobError::Internal(format!(
            "placed kickoff '{}' no-effect rejection has an empty error",
            obligation.input_id
        )));
    }
    let exact = match kickoff.phase {
        crate::roster::MobMemberKickoffPhase::Failed => kickoff.error.as_deref() == Some(error),
        // Cancellation wins only the public lifecycle projection; retain the
        // exact authenticated rejection detail separately for replay.
        crate::roster::MobMemberKickoffPhase::Cancelled => kickoff.error.is_none(),
        _ => false,
    };
    if !exact {
        return Err(MobError::Internal(format!(
            "placed kickoff '{}' no-effect rejection does not match its lifecycle projection",
            obligation.input_id
        )));
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
fn apply_recovered_placed_kickoff_terminal(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    obligation: mob_dsl::PlacedKickoffObligation,
    resolution: &RecoveredPlacedKickoffResolution,
) -> Result<(), MobError> {
    if resolution.kickoff.phase == crate::roster::MobMemberKickoffPhase::Cancelled {
        apply_seeded_mob_input(
            authority,
            mob_dsl::MobMachineInput::KickoffCancelRequested {
                member_id: obligation.agent_identity.clone(),
            },
            "recover_cancelled_placed_kickoff_before_host_terminal",
        )?;
    }
    let input = match &resolution.outcome {
        crate::event::PlacedKickoffHostOutcomeEvent::InteractionComplete => {
            mob_dsl::MobMachineInput::ResolvePlacedKickoffStarted { obligation }
        }
        crate::event::PlacedKickoffHostOutcomeEvent::InteractionCallbackPending => {
            mob_dsl::MobMachineInput::ResolvePlacedKickoffCallbackPending { obligation }
        }
        crate::event::PlacedKickoffHostOutcomeEvent::InteractionFailed { error } => {
            mob_dsl::MobMachineInput::ResolvePlacedKickoffFailed {
                obligation,
                error: error.clone(),
            }
        }
        crate::event::PlacedKickoffHostOutcomeEvent::InteractionCancelled => {
            mob_dsl::MobMachineInput::ResolvePlacedKickoffCancelled { obligation }
        }
    };
    apply_seeded_mob_input(authority, input, "recover_placed_kickoff_terminal")
}

#[cfg(feature = "runtime-adapter")]
fn apply_recovered_placed_kickoff_rejection(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    obligation: mob_dsl::PlacedKickoffObligation,
    rejection: &RecoveredPlacedKickoffRejection,
) -> Result<(), MobError> {
    if rejection.kickoff.phase == crate::roster::MobMemberKickoffPhase::Cancelled {
        apply_seeded_mob_input(
            authority,
            mob_dsl::MobMachineInput::KickoffCancelRequested {
                member_id: obligation.agent_identity.clone(),
            },
            "recover_cancelled_placed_kickoff_rejection",
        )?;
    }
    apply_seeded_mob_input(
        authority,
        mob_dsl::MobMachineInput::RejectPlacedKickoffBeforeAdmission {
            obligation,
            error: rejection.error.clone(),
        },
        "recover_placed_kickoff_rejection",
    )
}

#[cfg(feature = "runtime-adapter")]
fn finalized_placed_kickoff_recovery_signal(
    obligation: mob_dsl::PlacedKickoffObligation,
    chain: &PlacedKickoffRecoveryChain,
) -> Result<Option<mob_dsl::MobMachineSignal>, MobError> {
    let resolved_outcome = |resolution: &RecoveredPlacedKickoffResolution| match &resolution.outcome
    {
        crate::event::PlacedKickoffHostOutcomeEvent::InteractionComplete => {
            (mob_dsl::PlacedKickoffOutcomeKind::Started, None)
        }
        crate::event::PlacedKickoffHostOutcomeEvent::InteractionCallbackPending => {
            (mob_dsl::PlacedKickoffOutcomeKind::CallbackPending, None)
        }
        crate::event::PlacedKickoffHostOutcomeEvent::InteractionFailed { error } => (
            mob_dsl::PlacedKickoffOutcomeKind::Failed,
            Some(error.clone()),
        ),
        crate::event::PlacedKickoffHostOutcomeEvent::InteractionCancelled => {
            (mob_dsl::PlacedKickoffOutcomeKind::Cancelled, None)
        }
    };
    let (outcome_kind, outcome_error, closure_kind) =
        if let Some(rejection) = chain.rejected_no_effect.as_ref() {
            (
                mob_dsl::PlacedKickoffOutcomeKind::RejectedNoEffect,
                Some(rejection.error.clone()),
                mob_dsl::PlacedKickoffClosureKind::RejectedNoEffect,
            )
        } else if chain.acknowledged {
            let resolution = chain.resolved.as_ref().ok_or_else(|| {
                MobError::Internal(format!(
                    "placed kickoff '{}' ACK recovery has no exact host terminal",
                    obligation.input_id.0
                ))
            })?;
            let (kind, error) = resolved_outcome(resolution);
            (kind, error, mob_dsl::PlacedKickoffClosureKind::Acknowledged)
        } else if chain.disposed.is_some() {
            let (kind, error) = chain.resolved.as_ref().map_or(
                (mob_dsl::PlacedKickoffOutcomeKind::Disposed, None),
                resolved_outcome,
            );
            (kind, error, mob_dsl::PlacedKickoffClosureKind::Disposed)
        } else {
            return Ok(None);
        };
    Ok(Some(
        mob_dsl::MobMachineSignal::RecoverFinalizedPlacedKickoff {
            obligation,
            outcome_kind,
            outcome_error,
            closure_kind,
        },
    ))
}

/// Join exact current committed autonomous carriers to their public custody
/// chains. Missing Record+Starting projection is repaired atomically from the
/// private carrier before any actor/replay worker exists.
#[cfg(feature = "runtime-adapter")]
async fn recover_placed_kickoff_outcome_custody(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    event_store: &Arc<dyn crate::store::MobEventStore>,
    mob_id: &MobId,
    events: &mut Vec<crate::event::MobEvent>,
    carriers: &PlacedCarrierRecoveryPlan,
) -> Result<(), MobError> {
    let mut epoch_start = events
        .iter()
        .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
        .map_or(0, |position| position + 1);
    let mut chains = collect_placed_kickoff_recovery_chains(&events[epoch_start..])?;
    let mut current_keys = BTreeSet::<PlacedKickoffRecoveryKey>::new();

    for entry in &carriers.entries {
        if entry.disposition != PlacedCarrierRecoveryDisposition::CommittedCurrent
            || entry.record.spec.profile.runtime_mode
                != meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost
        {
            continue;
        }
        let expected =
            super::placed_kickoff_reconciler::obligation_event_from_carrier(&entry.record)?;
        let key = PlacedKickoffRecoveryKey::from_event(&expected);
        let spawn_cursor = current_placed_spawn_event_cursor(events, &entry.record)?;
        if !current_keys.insert(key.clone()) {
            return Err(MobError::Internal(format!(
                "duplicate current placed kickoff carrier for '{}'",
                expected.agent_identity
            )));
        }
        if let Some(chain) = chains.get(&key) {
            if chain
                .recorded_cursor
                .is_none_or(|recorded_cursor| recorded_cursor <= spawn_cursor)
            {
                return Err(MobError::Internal(format!(
                    "placed kickoff '{}' Record does not follow its exact current MemberSpawned",
                    expected.input_id
                )));
            }
            continue;
        }
        let expected_turn_key = PlacedKickoffTurnKey::from(&expected);
        if chains
            .keys()
            .any(|candidate| PlacedKickoffTurnKey::from(candidate) == expected_turn_key)
        {
            return Err(MobError::Internal(format!(
                "current placed kickoff input '{}' conflicts with public history",
                expected.input_id
            )));
        }
        let latest_kickoff =
            latest_kickoff_after_spawn(events, spawn_cursor, &expected.agent_identity);
        if latest_kickoff.is_some_and(|kickoff| {
            matches!(
                kickoff.phase,
                crate::roster::MobMemberKickoffPhase::Started
                    | crate::roster::MobMemberKickoffPhase::CallbackPending
                    | crate::roster::MobMemberKickoffPhase::Failed
                    | crate::roster::MobMemberKickoffPhase::Cancelled
            )
        }) {
            // A terminal from the pre-custody implementation is already
            // settled. Never replay model-visible work merely to retrofit an
            // ACK protocol that did not exist when it ran.
            continue;
        }
        if let Some(kickoff) = latest_kickoff
            && kickoff
                .objective_id
                .is_some_and(|objective| objective != expected.objective_id)
        {
            return Err(MobError::Internal(format!(
                "placed kickoff '{}' lifecycle objective conflicts with its durable intent",
                expected.agent_identity
            )));
        }
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&expected.agent_identity);
        if let Some(existing) = authority
            .state()
            .member_kickoff_objective_ids
            .get(&dsl_identity)
            && existing != &expected.objective_id.to_string()
        {
            return Err(MobError::Internal(format!(
                "placed kickoff '{}' objective conflicts with its durable intent",
                expected.agent_identity
            )));
        }
        let kickoff = crate::roster::MobMemberKickoffSnapshot {
            objective_id: Some(expected.objective_id),
            phase: crate::roster::MobMemberKickoffPhase::Starting,
            error: None,
            updated_at: meerkat_core::time_compat::SystemTime::now(),
        };
        let batch = vec![
            NewMobEvent {
                mob_id: mob_id.clone(),
                timestamp: None,
                kind: MobEventKind::PlacedKickoffObligationRecorded {
                    obligation: expected.clone(),
                },
            },
            NewMobEvent {
                mob_id: mob_id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberKickoffUpdated {
                    member: expected.agent_identity.clone(),
                    kickoff,
                },
            },
        ];
        match event_store.append_batch(batch).await {
            Ok(appended) => events.extend(appended),
            Err(error) => {
                // Handle a write-then-error backend by re-reading before
                // failing. The actor must never start while Record durability
                // is uncertain.
                let replayed = event_store
                    .replay_all()
                    .await?
                    .into_iter()
                    .filter(|event| &event.mob_id == mob_id)
                    .collect::<Vec<_>>();
                let replayed_epoch_start = replayed
                    .iter()
                    .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
                    .map_or(0, |position| position + 1);
                let replayed_chains =
                    collect_placed_kickoff_recovery_chains(&replayed[replayed_epoch_start..])?;
                if !replayed_chains.contains_key(&key) {
                    return Err(MobError::Internal(format!(
                        "placed kickoff Record repair for '{}' failed and is not durably observable: {error}",
                        expected.agent_identity
                    )));
                }
                *events = replayed;
                epoch_start = replayed_epoch_start;
            }
        }
        chains = collect_placed_kickoff_recovery_chains(&events[epoch_start..])?;
    }

    // A nonfinal public chain with no current exact carrier has lost its only
    // replay-complete prompt or residency authority. Final historical chains
    // remain valid audit history after normal carrier deletion.
    for (key, chain) in &chains {
        let finalized =
            chain.acknowledged || chain.disposed.is_some() || chain.rejected_no_effect.is_some();
        if !finalized && !current_keys.contains(key) {
            return Err(MobError::Internal(format!(
                "nonfinal placed kickoff '{}' has no exact current committed carrier",
                key.input_id
            )));
        }
    }

    for entry in &carriers.entries {
        if entry.disposition != PlacedCarrierRecoveryDisposition::CommittedCurrent
            || entry.record.spec.profile.runtime_mode
                != meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost
        {
            continue;
        }
        let expected =
            super::placed_kickoff_reconciler::obligation_event_from_carrier(&entry.record)?;
        let key = PlacedKickoffRecoveryKey::from_event(&expected);
        let Some(chain) = chains.get(&key) else {
            // Settled legacy terminal, handled above.
            continue;
        };
        let spawn_cursor = current_placed_spawn_event_cursor(events, &entry.record)?;
        let latest_kickoff = latest_kickoff_after_spawn(
            events,
            spawn_cursor,
            &expected.agent_identity,
        )
        .ok_or_else(|| {
            MobError::Internal(format!(
                "placed kickoff '{}' custody has no generation-current lifecycle projection",
                expected.input_id
            ))
        })?;
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&expected.agent_identity);
        if let Some(existing) = authority
            .state()
            .member_kickoff_objective_ids
            .get(&dsl_identity)
            && existing != &expected.objective_id.to_string()
        {
            return Err(MobError::Internal(format!(
                "placed kickoff '{}' objective conflicts with its exact custody",
                expected.agent_identity
            )));
        }
        apply_seeded_mob_signal(
            authority,
            mob_dsl::MobMachineSignal::RecoverObjectiveBinding {
                member_id: dsl_identity.clone(),
                objective_id: expected.objective_id.to_string(),
            },
            "recover_placed_kickoff_objective",
        )?;
        let obligation = super::remote_flow_ticket::placed_kickoff_obligation_from_event(&expected);
        if let Some(finalized) =
            finalized_placed_kickoff_recovery_signal(obligation.clone(), chain)?
        {
            // Finalized audit/tombstone truth has no live host custody. Restore
            // it in one generated recovery transition so a revoked/dormant
            // carrier does not have to satisfy the live Start routing guards.
            apply_seeded_mob_signal(
                authority,
                mob_dsl::MobMachineSignal::RecoverMemberKickoff {
                    member_id: obligation.agent_identity.clone(),
                    phase: dsl_kickoff_phase(latest_kickoff.phase),
                    error: latest_kickoff.error.clone(),
                },
                "recover_finalized_placed_kickoff_public_terminal",
            )?;
            apply_seeded_mob_signal(authority, finalized, "recover_finalized_placed_kickoff")?;
            continue;
        }
        let recovered_cancelled =
            latest_kickoff.phase == crate::roster::MobMemberKickoffPhase::Cancelled;
        if chain.resolved.is_none()
            && chain.rejected_no_effect.is_none()
            && matches!(
                latest_kickoff.phase,
                crate::roster::MobMemberKickoffPhase::Started
                    | crate::roster::MobMemberKickoffPhase::CallbackPending
                    | crate::roster::MobMemberKickoffPhase::Failed
                    | crate::roster::MobMemberKickoffPhase::Cancelled
            )
            && !recovered_cancelled
        {
            return Err(MobError::Internal(format!(
                "pending placed kickoff '{}' conflicts with a non-cancelled terminal lifecycle projection",
                expected.input_id
            )));
        }
        // Public lifecycle replay may already show Starting or a terminal.
        // Rewind only the generated projection to Pending, then replay the
        // exact custody chain through the same guarded inputs as live code.
        apply_seeded_mob_signal(
            authority,
            mob_dsl::MobMachineSignal::RecoverMemberKickoff {
                member_id: dsl_identity,
                phase: mob_dsl::KickoffPhase::Pending,
                error: None,
            },
            "recover_placed_kickoff_pending_base",
        )?;
        apply_seeded_mob_input(
            authority,
            mob_dsl::MobMachineInput::StartPlacedKickoff {
                obligation: obligation.clone(),
            },
            "recover_placed_kickoff_record",
        )?;
        if recovered_cancelled && chain.resolved.is_none() && chain.rejected_no_effect.is_none() {
            apply_seeded_mob_input(
                authority,
                mob_dsl::MobMachineInput::KickoffCancelRequested {
                    member_id: mob_dsl::AgentIdentity::from_domain(&expected.agent_identity),
                },
                "recover_cancelled_pending_placed_kickoff",
            )?;
        }
        if let Some(resolution) = chain.resolved.as_ref() {
            validate_recovered_kickoff_resolution(
                &expected,
                &resolution.outcome,
                &resolution.kickoff,
            )?;
            apply_recovered_placed_kickoff_terminal(authority, obligation.clone(), resolution)?;
        } else if let Some(rejection) = chain.rejected_no_effect.as_ref() {
            validate_recovered_kickoff_rejection(&expected, &rejection.error, &rejection.kickoff)?;
            apply_recovered_placed_kickoff_rejection(authority, obligation.clone(), rejection)?;
        }
        if chain.acknowledged {
            apply_seeded_mob_input(
                authority,
                mob_dsl::MobMachineInput::AcknowledgePlacedKickoffOutcome {
                    obligation: obligation.clone(),
                },
                "recover_placed_kickoff_acknowledged",
            )?;
        }
        if chain.disposed.is_some() {
            apply_seeded_mob_input(
                authority,
                mob_dsl::MobMachineInput::DisposePlacedKickoffObligation { obligation },
                "recover_placed_kickoff_disposed",
            )?;
        }
    }
    Ok(())
}

pub(super) async fn converge_recovered_active_flow_run(
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
    reachability_observations: Arc<super::handle::ReachabilityObservations>,
    restore_diagnostics:
        Arc<RwLock<HashMap<AgentIdentity, super::handle::RestoreFailureDiagnostic>>>,
    runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    supervisor_bridge: Arc<MobSupervisorBridge>,
    command_tx: mpsc::Sender<super::scope_gate::RoutedMobCommand>,
    command_rx: mpsc::Receiver<super::scope_gate::RoutedMobCommand>,
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
            spawn_base_prompt_source: None,
            spawn_member_customizer: None,
            owner_bridge_session_create_authority: None,
            realtime_session_factory: None,
            controlling_acceptor: None,
            member_live_host: None,
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
            spawn_base_prompt_source: None,
            spawn_member_customizer: None,
            owner_bridge_session_create_authority: None,
            realtime_session_factory: None,
            controlling_acceptor: None,
            member_live_host: None,
        }
    }

    /// Compose the controlling-side reverse-lane acceptor (ADJ-P4-2). Cloned
    /// configs share one host-process `HostAcceptor` demux through which
    /// remote member hosts reach local members with cross-host edges.
    pub fn with_controlling_acceptor(mut self, config: ControllingAcceptorConfig) -> Self {
        self.controlling_acceptor = Some(config);
        self
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

    /// Inject the R3 case-3 base-prompt seam for placed spawns (ADJ-2):
    /// resolves the CONTROLLING host's assembled base system prompt for a
    /// remote spawn whose profile has no skills and no prompt override.
    /// Absent, such spawns fail typed at spec compile (never a silent
    /// Inherit).
    pub fn with_spawn_base_prompt_source(
        mut self,
        source: Arc<dyn super::spec_compiler::SpawnBasePromptSource>,
    ) -> Self {
        self.spawn_base_prompt_source = Some(source);
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

    /// Inject the LOCAL-branch live gateway for the identity-addressed
    /// `member_live_*` verbs (phase 6b, ADJ-P6B-1): the session-id-addressed
    /// entry to the ONE extracted live pipeline. Live-capable surfaces
    /// (rkat-rpc with a live transport; `rkat mob host` for its own mobs)
    /// pass their `ServiceMemberLiveHost` here (ADJ-P6B-16). Absent, local
    /// members' live verbs typed-reject `LiveTransportUnavailable`.
    pub fn with_member_live_host(
        mut self,
        live_host: Arc<dyn meerkat_runtime::member_live::MemberLiveHost>,
    ) -> Self {
        self.member_live_host = Some(live_host);
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
            notify_orchestrator_on_resume,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            spawn_base_prompt_source,
            spawn_member_customizer,
            owner_bridge_session_create_authority,
            realtime_session_factory,
            controlling_acceptor,
            member_live_host,
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
        Self::recover_host_bindings(
            storage.runtime_metadata.clone(),
            &definition.id,
            &mut dsl_authority,
            "create_after_insert_race",
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
            spawn_base_prompt_source,
            spawn_member_customizer,
            storage.realm_profiles.clone(),
            notify_orchestrator_on_resume,
            realtime_session_factory,
            controlling_acceptor,
            member_live_host,
        )
        .await
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
            spawn_base_prompt_source,
            spawn_member_customizer,
            owner_bridge_session_create_authority: _,
            realtime_session_factory,
            controlling_acceptor,
            member_live_host,
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
        #[allow(unused_mut)]
        let mut mob_events: Vec<_> = all_events
            .into_iter()
            .filter(|event| event.mob_id == definition.id)
            .collect();
        // Validate authority-operation journals before definition/spec sync
        // or any other recovery mutation/repair write.
        super::actor::validate_host_authority_anchor_epoch_boundaries(&mob_events)?;
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
        let mut initial_dsl_authority = Box::new(seed_mob_authority());
        recover_owner_bridge_session_authority_from_history(
            &mut initial_dsl_authority,
            &mob_events,
        )?;
        Self::prepare_recovered_host_binds(
            storage.runtime_metadata.clone(),
            storage.events.clone(),
            &definition.id,
            &mut mob_events,
            initial_dsl_authority.as_mut(),
        )
        .await?;
        Self::recover_host_binding_generation_highwaters(
            initial_dsl_authority.as_mut(),
            &mob_events,
            "resume_before_placed_event_repair",
        )?;
        Self::recover_confirmed_host_binding_revocations(
            initial_dsl_authority.as_mut(),
            &mob_events,
            "resume_before_placed_event_repair",
        )?;
        Self::recover_host_bindings(
            storage.runtime_metadata.clone(),
            &definition.id,
            initial_dsl_authority.as_mut(),
            "resume_before_placed_event_repair",
        )
        .await?;
        // Read-only carrier/event classification is the first placed-member
        // recovery step.  A missing-event repair is intentionally deferred
        // until historical machine recovery and exact operation recovery have
        // both succeeded.
        let (placed_recovery, placed_event_repairs) =
            prepare_placed_spawn_recovery(&storage.runtime_metadata, &definition.id, &mob_events)
                .await?;
        // Select the current durable epoch (after the last MobReset, or all
        // events if no reset has occurred). Do this before writing abandonment
        // repairs so destroy-finalizing storage remains mutation-free.
        let preliminary_epoch_start = mob_events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |pos| pos + 1);
        let destroy_storage_finalizing = mob_events[preliminary_epoch_start..]
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobDestroyStorageFinalizing));
        if !destroy_storage_finalizing {
            ensure_recovered_respawn_topology_abandonments(
                storage.events.as_ref(),
                &definition.id,
                &mut mob_events,
                &placed_recovery.current_committed_event_keys,
            )
            .await?;
        }
        let epoch_start = mob_events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |pos| pos + 1);
        let epoch_events = &mob_events[epoch_start..];
        // Supervisor-authority recovery follows exact marker repair so all
        // downstream projections and machine seeding see one durable epoch.
        // Ordering pin (§8, DEC-P5P-7): grant recovery rides the live
        // `GrantOperatorScopes` input, whose guard is
        // `lifecycle_phase == Running` — so it must run while the
        // freshly-seeded authority is still Running, BEFORE the event replay
        // below re-applies `MobCompleted`/`MobDestroying`. A
        // completed-then-restored mob keeps its grants this way. Skipped
        // under the destroy fence like the sibling metadata recoveries:
        // destroy-finalizing storage must not re-mint facts mid-scrub.
        if !destroy_storage_finalizing {
            Self::recover_operator_grants(
                storage.runtime_metadata.clone(),
                &definition.id,
                initial_dsl_authority.as_mut(),
            )
            .await?;
        }
        let resumed_state = recovered_public_phase_from_events(epoch_events);
        let recovered_host_revocations = if destroy_storage_finalizing {
            Vec::new()
        } else {
            Self::prepare_recovered_host_revocations(
                storage.runtime_metadata.clone(),
                storage.events.clone(),
                &definition.id,
                epoch_events,
            )
            .await?
        };
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
        recover_cleanup_only_placed_carrier_signals(
            initial_dsl_authority.as_mut(),
            &placed_recovery,
        )?;
        // One provisioner owns the exact attachment sidecars for the whole
        // recovered runtime. Placed-carrier cleanup, Running reconciliation,
        // and the eventual actor must never materialize through temporary
        // provisioners that the actor cannot subsequently observe.
        let runtime_provisioner = Arc::new(
            MultiBackendProvisioner::new(
                session_service.clone(),
                runtime_adapter.clone(),
                workgraph_service.clone(),
                definition.backend.external.clone(),
                Arc::clone(&supervisor_bridge),
            )
            .with_binding_persistence(definition.id.clone(), storage.runtime_metadata.clone()),
        );
        drive_recovered_placed_carrier_cleanup(
            initial_dsl_authority.as_mut(),
            &storage.runtime_metadata,
            &definition.id,
            &placed_recovery,
            runtime_provisioner.as_ref(),
            Arc::clone(&supervisor_bridge),
        )
        .await?;

        // Generic event replay remains authoritative for local members and
        // retired history. It skips only the exact current placed projection;
        // the canonical committed carrier installs that incarnation in one
        // recovery signal after historical generations establish high-water.
        seed_mob_authority_sync_from_events(
            &mut initial_dsl_authority,
            epoch_events,
            &definition,
            resumed_state == MobState::Completed,
            &placed_recovery.current_committed_event_keys,
        )?;
        recover_current_committed_placed_carrier_signals(
            initial_dsl_authority.as_mut(),
            &placed_recovery,
        )?;

        // A carrier alone may rebuild private machine state, but public roster
        // history is repaired only after that exact machine tuple has rebuilt
        // (or validated) the SAME pre-minted owner operation.  Append and
        // reload are therefore the final recovery step before projection.
        let placed_recovery = commit_placed_spawn_event_repairs(
            &storage.events,
            &storage.runtime_metadata,
            &definition.id,
            &mut mob_events,
            placed_recovery,
            placed_event_repairs,
            initial_dsl_authority.as_ref(),
            runtime_provisioner.as_ref(),
        )
        .await?;
        recover_placed_kickoff_outcome_custody(
            initial_dsl_authority.as_mut(),
            &storage.events,
            &definition.id,
            &mut mob_events,
            &placed_recovery,
        )
        .await?;
        let recovered_completion_lifecycle_intent = recover_placed_completion_outcome_custody(
            initial_dsl_authority.as_mut(),
            &storage.events,
            &definition.id,
            &mut mob_events,
        )
        .await?;
        let repaired_epoch_start = mob_events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |pos| pos + 1);
        if matches!(
            resumed_state,
            MobState::Running | MobState::Stopped | MobState::Completed
        ) {
            recover_pending_member_retirements(
                initial_dsl_authority.as_mut(),
                &mob_events[repaired_epoch_start..],
            )?;
        }
        let repaired_epoch_events = &mob_events[repaired_epoch_start..];

        // Roster is a projection only. Build it after the authoritative
        // carrier/event reconciliation and machine recovery have succeeded.
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
        let reachability_observations =
            Arc::new(super::handle::ReachabilityObservations::default());
        // Preview phase watch so the preview handle can answer status()
        // before the actor spawns. The real actor-side sender replaces
        // this once start_runtime_with_components owns the final pair.
        let (_preview_phase_tx, preview_phase_rx) = tokio::sync::watch::channel(resumed_state);
        let mut wiring = RuntimeWiring {
            roster: roster_state.clone(),
            dsl_authority: initial_dsl_authority,
            machine_state_watch_tx,
            reachability_observations: Arc::clone(&reachability_observations),
            restore_diagnostics: restore_diagnostics.clone(),
            runtime_metadata: storage.runtime_metadata.clone(),
            supervisor_bridge: supervisor_bridge.clone(),
            command_tx: command_tx.clone(),
            command_rx,
        };
        let preview_handle = MobHandle {
            // Explicit launch-site mint (A16): the building process IS the
            // owning operator; surfaces rebind clones per console principal.
            command_authority: crate::control_policy::CommandAuthority::principal(
                crate::control_policy::MobControlPrincipal::Owner,
            ),
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
            reachability_observations,
            phase_watch_rx: preview_phase_rx,
            realtime_session_factory: realtime_session_factory.clone(),
        };
        // session_service is still live here (not consumed until start_runtime_with_components)

        let seeded_topology_epoch = Arc::new(std::sync::atomic::AtomicU64::new(
            wiring.dsl_authority.state().topology_epoch,
        ));

        let mut per_spawn_external_tools_seed = BTreeMap::new();
        if resumed_state == MobState::Running
            && recovered_completion_lifecycle_intent.is_none()
            && !super::actor::lifecycle_origin_fenced(wiring.dsl_authority.state())
        {
            Self::reconcile_resume(
                &definition,
                repaired_epoch_events,
                &mut roster,
                &session_service,
                runtime_provisioner.as_ref(),
                &runtime_adapter,
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
            // T3 (ADJ-P4-1): re-derive cross-host route-install obligations
            // from the recovered durable graph facts. RECORD only — the
            // machine's `RecordRouteInstall` guards demand Running phase and
            // recovered placement/host facts, which `reconcile_resume` has
            // just replayed. Realization is deferred off the recovery path
            // (the first authenticated periodic HostStatus, host rebind, or
            // operator drive drains them), so resume never blocks on bridge
            // round trips.
            record_recovered_route_install_obligations(
                wiring.dsl_authority.as_mut(),
                &definition.id,
            );
        }

        let mut deferred_remote_intent_runs = BTreeSet::new();
        let mut recovered_private_remote_turns = Vec::new();
        if matches!(
            resumed_state,
            MobState::Running | MobState::Stopped | MobState::Completed
        ) {
            // Remote outcome custody depends on recovered placement and exact
            // generation/fence material. Reconstruct it for every recoverable
            // lifecycle phase; Stopped/Completed must not orphan host rows.
            let remote_turn_material =
                super::remote_turn_reconciler::prepare_remote_turn_recovery_material(
                    storage.runs.clone(),
                    storage.events.clone(),
                    &definition.id,
                    repaired_epoch_events,
                    true,
                )
                .await?;
            deferred_remote_intent_runs = remote_turn_material.deferred_run_ids.clone();
            recovered_private_remote_turns = remote_turn_material
                .intents
                .iter()
                .map(|intent| intent.obligation.clone())
                .collect();
            // Startup Record repair appends to the event store after
            // `repaired_epoch_events` was sliced. Reload so custody recovery
            // validates the repaired public chain, never the stale pre-repair
            // snapshot that could lose finalized privacy rows on retry.
            let remote_turn_events = storage.events.replay_all().await?;
            let remote_turn_mob_events = remote_turn_events
                .iter()
                .filter(|event| event.mob_id == definition.id)
                .collect::<Vec<_>>();
            let remote_turn_epoch_start = remote_turn_mob_events
                .iter()
                .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
                .map_or(0, |position| position + 1);
            let remote_turn_epoch_events = remote_turn_mob_events[remote_turn_epoch_start..]
                .iter()
                .copied()
                .cloned()
                .collect::<Vec<_>>();
            recover_remote_turn_outcome_custody(
                wiring.dsl_authority.as_mut(),
                &remote_turn_epoch_events,
                &remote_turn_material.intents,
                &remote_turn_material.receipts,
            )?;
            // ACK/Dispose makes private material eligible for privacy
            // cleanup, but only the full recovery validator above proves the
            // public finalizer chain is coherent. Delete receipt-first after
            // that proof so corruption never destroys its own evidence.
            super::remote_turn_reconciler::apply_remote_turn_finalized_privacy_cleanup(
                storage.runs.clone(),
                &remote_turn_material.finalized_privacy_cleanup,
            )
            .await?;
        }
        // Persisted flow seeds are recovery facts, not fresh origin. Rebuild
        // them while the authority is still in its open seed phase, before a
        // recovered lifecycle intent closes every fresh RunFlow/Create* arm.
        seed_mob_authority_sync_from_flow_runs(
            &mut wiring.dsl_authority,
            storage.runs.clone(),
            storage.events.clone(),
            &definition.id,
            repaired_epoch_events,
            &deferred_remote_intent_runs,
        )
        .await?;
        if resumed_state == MobState::Running {
            apply_recovered_placed_completion_lifecycle_intent(
                wiring.dsl_authority.as_mut(),
                recovered_completion_lifecycle_intent,
            )?;
        }
        if matches!(resumed_state, MobState::Stopped | MobState::Completed) {
            finish_seeded_mob_authority_phase(wiring.dsl_authority.as_mut(), resumed_state)?;
            apply_recovered_placed_completion_lifecycle_intent(
                wiring.dsl_authority.as_mut(),
                recovered_completion_lifecycle_intent,
            )?;
        }

        let restore_diagnostics_snapshot = preview_handle.restore_diagnostics.read().await.clone();
        seed_mob_authority_restore_failures(
            &mut wiring.dsl_authority,
            &restore_diagnostics_snapshot,
        )?;
        let _ = wiring
            .machine_state_watch_tx
            .send(wiring.dsl_authority.state().clone());
        *wiring.roster.write().await = RosterAuthority::from_roster(roster);

        // Rebuild the actor's O(1) lifecycle idempotency indexes from the
        // same current reset epoch that seeded MobMachine. Leaving these
        // empty would make the automatic retirement retry append a duplicate
        // Started carrier even though replay already restored Retiring.
        let mut retired_event_index = HashSet::new();
        let mut retirement_started_event_index = HashSet::new();
        for event in repaired_epoch_events {
            match &event.kind {
                MobEventKind::MemberRetirementStarted {
                    agent_identity,
                    generation,
                    ..
                } => {
                    let key = format!("{}:{}", agent_identity, generation.get());
                    retirement_started_event_index.insert(key);
                }
                MobEventKind::MemberRetired {
                    agent_identity,
                    generation,
                    ..
                } => {
                    retired_event_index.insert(format!("{}:{}", agent_identity, generation.get()));
                }
                _ => {}
            }
        }

        Self::start_runtime_with_components(
            definition,
            wiring,
            storage.events.clone(),
            storage.runs.clone(),
            session_service,
            runtime_adapter,
            runtime_provisioner,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            spawn_base_prompt_source,
            spawn_member_customizer,
            storage.realm_profiles.clone(),
            notify_orchestrator_on_resume,
            per_spawn_external_tools_seed,
            retired_event_index,
            retirement_started_event_index,
            // Respawn is a live helper composition, not a durable replacement
            // operation. A recovered Started-without-terminal worker therefore
            // completes ordinary retirement; only a successor already present
            // in the replay stream consumes a temporary topology hold.
            HashSet::new(),
            deferred_remote_intent_runs,
            recovered_private_remote_turns,
            recovered_host_revocations,
            placed_recovery.next_carrier_fence_token,
            !destroy_storage_finalizing,
            realtime_session_factory,
            controlling_acceptor,
            member_live_host,
        )
        .await
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

    /// Recover unfinished controller BindHost ceremonies before placed
    /// carrier recovery. A Started-only row restores the exact pending
    /// ordinary/replacement region from event truth. A Confirmed ACK is
    /// sufficient to insert the byte-exact local authority record without a
    /// bootstrap-token replay; conflict never overwrites. Once that record is
    /// durable, the existing host-record recovery restores Bound and this
    /// helper closes the structural operation with Completed.
    async fn prepare_recovered_host_binds(
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        event_store: Arc<dyn crate::store::MobEventStore>,
        mob_id: &MobId,
        mob_events: &mut Vec<crate::event::MobEvent>,
        authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    ) -> Result<(), MobError> {
        let epoch_start = mob_events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |position| position + 1);
        let epoch_events = &mob_events[epoch_start..];
        let anchors = super::actor::pending_host_bind_anchors(epoch_events)?;
        if anchors.is_empty() {
            return Ok(());
        }
        if epoch_events
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobDestroyStorageFinalizing))
        {
            return Err(MobError::Internal(format!(
                "mob '{mob_id}' entered destroy storage finalization with unfinished host-bind authority"
            )));
        }

        for anchor in anchors {
            match anchor.confirmed_authority {
                Some(record) => {
                    let recovery_authority =
                        crate::store::MobHostAuthorityPersistenceAuthority::from_confirmed_bind_anchor(
                            &anchor.request,
                            &record,
                        )?;
                    match runtime_metadata
                        .load_mob_host_authority(mob_id, &record.host_id)
                        .await?
                    {
                        Some(existing) if existing == record => {}
                        Some(_) => {
                            return Err(MobError::Internal(format!(
                                "confirmed host bind '{}' conflicts with the durable authority row for host '{}'",
                                anchor.operation_id, record.host_id
                            )));
                        }
                        None => {
                            let inserted = runtime_metadata
                                .put_mob_host_authority_if_absent(
                                    mob_id,
                                    &record,
                                    &recovery_authority,
                                )
                                .await?;
                            if !inserted {
                                let existing = runtime_metadata
                                    .load_mob_host_authority(mob_id, &record.host_id)
                                    .await?;
                                if existing.as_ref() != Some(&record) {
                                    return Err(MobError::Internal(format!(
                                        "confirmed host bind '{}' lost an insert race to conflicting authority for host '{}'",
                                        anchor.operation_id, record.host_id
                                    )));
                                }
                            }
                        }
                    }
                    let completed = event_store
                        .append(crate::event::NewMobEvent {
                            mob_id: mob_id.clone(),
                            timestamp: None,
                            kind: MobEventKind::RemoteHostBindCompleted {
                                operation_id: anchor.operation_id,
                                host_id: record.host_id,
                                authority_epoch: record.authority_epoch,
                                binding_generation: record.binding_generation,
                            },
                        })
                        .await?;
                    mob_events.push(completed);
                }
                None => {
                    if runtime_metadata
                        .load_mob_host_authority(mob_id, &anchor.request.host_id)
                        .await?
                        .is_some()
                    {
                        return Err(MobError::Internal(format!(
                            "unconfirmed host bind '{}' conflicts with an active durable authority row for host '{}'",
                            anchor.operation_id, anchor.request.host_id
                        )));
                    }
                    authority
                        .apply_signal(
                            crate::machines::mob_machine::MobMachineSignal::RecoverHostBindRequest {
                                host_id: crate::machines::mob_machine::HostId::from(
                                    anchor.request.host_id.clone(),
                                ),
                                expected_endpoint: crate::machines::mob_machine::PeerAddress::from(
                                    anchor.request.endpoint.clone(),
                                ),
                                binding_generation: anchor.request.binding_generation,
                                replacement: anchor.request.replacement,
                            },
                        )
                        .map(|_| ())
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "generated host bind request rejected recovery for host '{}' operation '{}' : {error}",
                                anchor.request.host_id, anchor.operation_id
                            ))
                        })?;
                }
            }
        }
        Ok(())
    }

    async fn prepare_recovered_host_revocations(
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        event_store: Arc<dyn crate::store::MobEventStore>,
        mob_id: &MobId,
        epoch_events: &[crate::event::MobEvent],
    ) -> Result<Vec<String>, MobError> {
        let anchors = super::actor::pending_host_revoke_anchors(epoch_events)?;
        let mut retry_hosts = Vec::new();
        for anchor in anchors {
            let current = runtime_metadata
                .load_mob_host_authority(mob_id, &anchor.host_id)
                .await?;
            if current.as_ref().is_some_and(|record| {
                record.authority_epoch == anchor.epoch
                    && record.binding_generation == anchor.binding_generation
            }) {
                retry_hosts.push(anchor.host_id);
                continue;
            }
            if !anchor.confirmed {
                return Err(MobError::Internal(format!(
                    "unfinished host revoke '{}' lost its host authority record before a durable remote confirmation",
                    anchor.operation_id,
                )));
            }
            // The local authority row is already absent (or a later binding
            // replaced it), so the old operation's local terminal holds. Close
            // the exact anchor synchronously before exposing the resumed handle;
            // this prevents it from targeting a later same-epoch rebind.
            event_store
                .append(crate::event::NewMobEvent {
                    mob_id: mob_id.clone(),
                    timestamp: None,
                    kind: MobEventKind::RemoteHostRevokeCompleted {
                        operation_id: anchor.operation_id,
                        host_id: anchor.host_id,
                        epoch: anchor.epoch,
                        binding_generation: anchor.binding_generation,
                    },
                })
                .await?;
        }
        Ok(retry_hosts)
    }

    /// Restore controlling-side host binding facts (FLAG-3(a)) from the
    /// durable `MobHostAuthorityRecord` family into the generated MobMachine
    /// authority via the `RecoverHostBinding` signal — the exact sibling of
    /// [`Self::recover_supervisor_authority`]. Without this, a controlling
    /// restart orphans bound hosts into an operator dead-end (the host says
    /// `AlreadyBound`; the controlling side forgot the host existed, §9).
    async fn recover_host_bindings(
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        mob_id: &MobId,
        authority: &mut crate::machines::mob_machine::MobMachineAuthority,
        context: &str,
    ) -> Result<(), MobError> {
        for (host_id, binding_generation) in runtime_metadata
            .list_mob_host_binding_generation_highwaters(mob_id)
            .await?
        {
            if binding_generation == 0 {
                continue;
            }
            authority
                .apply_signal(
                    crate::machines::mob_machine::MobMachineSignal::RecoverHostBindingGenerationHighwater {
                        host_id: crate::machines::mob_machine::HostId::from(host_id.clone()),
                        binding_generation,
                    },
                )
                .map(|_| ())
                .map_err(|error| {
                    MobError::Internal(format!(
                        "generated durable host binding generation highwater rejected recovery for host '{host_id}' ({context}): {error}"
                    ))
                })?;
        }
        for record in runtime_metadata.list_mob_host_authorities(mob_id).await? {
            authority
                .apply_signal(record.dsl_recover_signal())
                .map(|_| ())
                .map_err(|error| {
                    MobError::Internal(format!(
                        "generated host binding authority rejected recovery for host '{}' ({context}): {error}",
                        record.host_id
                    ))
                })?;
        }
        Ok(())
    }

    /// Restore the controller-side generation tombstones retained by durable
    /// revoke intents. `RemoteHostRevokeStarted` is the earliest durable
    /// point at which a generation has been consumed remotely, so every such
    /// row contributes to the per-host highwater even when the active host
    /// authority row was later deleted.
    fn recover_host_binding_generation_highwaters(
        authority: &mut crate::machines::mob_machine::MobMachineAuthority,
        events: &[crate::event::MobEvent],
        context: &str,
    ) -> Result<(), MobError> {
        let mut highwaters = BTreeMap::<String, u64>::new();
        let epoch_start = events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |position| position + 1);
        for event in &events[epoch_start..] {
            let tuple = match &event.kind {
                MobEventKind::RemoteHostBindStarted { request, .. } => {
                    Some((&request.host_id, request.binding_generation))
                }
                MobEventKind::RemoteHostRevokeStarted {
                    host_id,
                    binding_generation,
                    ..
                } => Some((host_id, *binding_generation)),
                _ => None,
            };
            let Some((host_id, binding_generation)) = tuple else {
                continue;
            };
            if binding_generation == 0 {
                continue;
            }
            highwaters
                .entry(host_id.clone())
                .and_modify(|current| *current = (*current).max(binding_generation))
                .or_insert(binding_generation);
        }
        for (host_id, binding_generation) in highwaters {
            authority
                .apply_signal(
                    crate::machines::mob_machine::MobMachineSignal::RecoverHostBindingGenerationHighwater {
                        host_id: crate::machines::mob_machine::HostId::from(host_id.clone()),
                        binding_generation,
                    },
                )
                .map(|_| ())
                .map_err(|error| {
                    MobError::Internal(format!(
                        "generated host binding generation highwater rejected recovery for host '{host_id}' ({context}): {error}"
                    ))
                })?;
        }
        Ok(())
    }

    /// Restore only authenticated revoke terminals. A Started/highwater row
    /// fences reuse but is not proof that a committed G carrier was disposed;
    /// dormant ordinary retire/destroy requires this exact tombstone.
    fn recover_confirmed_host_binding_revocations(
        authority: &mut crate::machines::mob_machine::MobMachineAuthority,
        events: &[crate::event::MobEvent],
        context: &str,
    ) -> Result<(), MobError> {
        let tombstones = events
            .iter()
            .filter_map(|event| match &event.kind {
                MobEventKind::RemoteHostRevokeConfirmed {
                    host_id,
                    binding_generation,
                    ..
                }
                | MobEventKind::RemoteHostRevokeCompleted {
                    host_id,
                    binding_generation,
                    ..
                } if *binding_generation > 0 => Some((host_id.clone(), *binding_generation)),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        for (host_id, binding_generation) in tombstones {
            authority
                .apply_signal(
                    crate::machines::mob_machine::MobMachineSignal::RecoverConfirmedHostBindingRevocation {
                        host_id: crate::machines::mob_machine::HostId::from(host_id.clone()),
                        binding_generation,
                    },
                )
                .map(|_| ())
                .map_err(|error| {
                    MobError::Internal(format!(
                        "generated confirmed host binding revocation rejected recovery for host '{host_id}' generation {binding_generation} ({context}): {error}"
                    ))
                })?;
        }
        Ok(())
    }

    /// Restore principal control-scope grants (§8) from the durable
    /// [`crate::store::MobOperatorGrantRecord`] rows into the generated
    /// MobMachine authority — the sibling of [`Self::recover_host_bindings`].
    ///
    /// The frozen catalog has no grant recovery SIGNAL; recovery rides the
    /// live `GrantOperatorScopes` input (the placed-member posture: recovery
    /// rides the same admission ladder the live write walked). The caller
    /// must invoke this while the seeded authority is still `Running` —
    /// BEFORE `seed_mob_authority_sync_from_events` replays lifecycle events
    /// — because the grant transition guards `lifecycle_phase == Running`.
    /// Expiry replays verbatim: only the enforcement seam evaluates it, so a
    /// restored mob's expired grants stay expired with zero persisted
    /// derived state.
    async fn recover_operator_grants(
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
        mob_id: &MobId,
        authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    ) -> Result<(), MobError> {
        for record in runtime_metadata.list_mob_operator_grants(mob_id).await? {
            apply_seeded_mob_input(
                authority,
                record.dsl_grant_input(),
                "recover_operator_grant",
            )?;
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
        epoch_events: &[crate::event::MobEvent],
        roster: &mut Roster,
        session_service: &Arc<dyn MobSessionService>,
        provisioner: &MultiBackendProvisioner,
        runtime_adapter: &RuntimeAdapterOption,
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
        let legacy_recovered_session_bindings =
            recovered_session_bindings_without_endpoint(epoch_events);
        let mut new_recovered_session_bindings_without_endpoint = HashSet::new();
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
        let machine_state = dsl_authority.state();
        let host_owned_runtime_ids = machine_state
            .member_placement
            .keys()
            .filter_map(|identity| machine_state.identity_to_runtime.get(identity))
            .cloned()
            .collect::<HashSet<_>>();
        let machine_session_ids = machine_state
            .member_session_bindings
            .iter()
            // Host-materialized session ids belong to the remote member host
            // and must not suppress controller-local orphan reconciliation.
            .filter(|(identity, _)| !machine_state.member_placement.contains_key(*identity))
            .map(|(_, session_id)| session_id)
            // A releasing retirement-start deliberately removes the ordinary
            // member binding, but its exact session remains machine-owned by
            // the durable pending-retirement obligation. Treat that session as
            // claimed during resume so orphan reconciliation cannot archive it
            // before the routed retirement retry runs.
            .chain(
                machine_state
                    .runtime_retire_pending_sessions
                    .iter()
                    .filter(|(runtime_id, _)| !host_owned_runtime_ids.contains(*runtime_id))
                    .map(|(_, session_id)| session_id),
            )
            .map(|session_id| {
                meerkat_core::types::SessionId::parse(&session_id.0).map_err(|error| {
                    MobError::Internal(format!(
                        "resume census found invalid machine-owned session binding '{}': {error}",
                        session_id.0
                    ))
                })
            })
            .collect::<Result<std::collections::HashSet<_>, MobError>>()?;

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
            // §19.L5 placement gate (gotcha 12): HostMaterialized members
            // HAVE machine session bindings (from the materialize ack) and
            // are never in the local census; without this gate they would be
            // locally recreated from a snapshot that does not exist in this
            // realm. Placed members reconcile via the (re)bind HostStatus
            // sweep + ReleaseMember-at-stale-fence + host-autonomous revival.
            if dsl_authority
                .state()
                .member_placement
                .contains_key(&dsl_identity)
            {
                continue;
            }
            let Some(machine_session_id) = dsl_authority
                .state()
                .member_session_bindings
                .get(&dsl_identity)
            else {
                continue;
            };
            let mut bridge_session_id = meerkat_core::types::SessionId::parse(
                &machine_session_id.0,
            )
            .map_err(|error| {
                MobError::Internal(format!(
                    "resume restore found invalid current session binding '{}' for '{}': {error}",
                    machine_session_id.0, entry.agent_identity
                ))
            })?;
            // Every durable local session crosses the explicit replacement
            // seam, even when the service actor is already absent: the
            // machine can still retain the old exact attachment and a fresh
            // provisioner must never adopt it. The roster may already carry a
            // BackendPeer endpoint alongside this local session binding, so
            // the machine-owned session id above—not the projection variant—
            // is the ownership fact. Runtime-less/nonpersistent services
            // retain an existing live realization as before.
            let replaced_for_explicit_resume = if session_service.supports_persistent_sessions() {
                provisioner
                    .retire_exact_attachment_for_explicit_resume(&bridge_session_id)
                    .await?
            } else {
                false
            };
            if active_ids.contains(&bridge_session_id) && !replaced_for_explicit_resume {
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
            restore_spec.model_override = entry.effective_model_override.clone();
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
            let restore_model_override = restore_spec.model_override.clone();
            let restore_labels = restore_spec
                .labels
                .clone()
                .unwrap_or_else(|| entry.labels.clone());
            if let Some(roster_entry) = roster.get_by_identity_mut(&entry.agent_identity) {
                roster_entry.effective_profile_override = restore_profile_override.clone();
                roster_entry.effective_model_override = restore_model_override.clone();
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
                            let replacement_rebuilt = provisioner
                                .retire_exact_attachment_for_explicit_resume(
                                    &replacement_session_id,
                                )
                                .await?;
                            let reuse_active_replacement = active_ids
                                .contains(&replacement_session_id)
                                && !replacement_rebuilt;
                            let recovered_endpoint = if reuse_active_replacement {
                                let member_ref = MemberRef::from_bridge_session_id(
                                    replacement_session_id.clone(),
                                );
                                let fallback_name = super::actor::render_member_comms_name(
                                    definition.id.as_str(),
                                    entry.role.as_str(),
                                    entry.agent_identity.as_str(),
                                )?;
                                match provisioner.comms_runtime(&member_ref).await {
                                    Some(runtime) => super::provisioner::SessionBackend::
                                        trusted_peer_spec_from_runtime(
                                            &fallback_name,
                                            runtime.as_ref(),
                                        )?,
                                    None => None,
                                }
                            } else {
                                None
                            };
                            let recovered_endpoint_missing = recovered_endpoint.is_none();
                            append_recovered_session_binding(
                                dsl_authority,
                                &tool_handle.events,
                                &definition.id,
                                entry,
                                &replacement_session_id,
                                recovered_endpoint,
                                "resume_repoint_missing_member_session_binding",
                            )
                            .await?;
                            if recovered_endpoint_missing {
                                new_recovered_session_bindings_without_endpoint
                                    .insert(entry.agent_identity.clone());
                            }
                            bridge_session_id = replacement_session_id;
                            let _ = roster.set_bridge_session_id(
                                &entry.agent_identity,
                                bridge_session_id.clone(),
                            );
                            if reuse_active_replacement {
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
                if let Some(model) = restore_model_override.as_ref() {
                    profile.model.clone_from(model);
                }
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
                    None,
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
                if let Some(reconcile_client) = default_llm_client.clone() {
                    resumed_config.llm_client_override = Some(reconcile_client);
                }
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
                        session_origin: super::provisioner::ProvisionSessionOrigin::ResumedDurable,
                        binding: crate::RuntimeBinding::Session,
                        peer_name,
                        owner_bridge_session_id: None,
                        ops_registry: None,
                        generated_self_owned_operation_owner: Some(
                            generated_self_owned_operation_owner,
                        ),
                        runtime_revival_intent: super::provisioner::RuntimeRevivalIntent::None,
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
            if let Some(model) = restore_model_override.as_ref() {
                profile.model.clone_from(model);
            }
            if restore_spec.inherited_tool_filter.is_some() && restore_profile_override.is_none() {
                build::open_profile_tool_categories_for_inherited_filter(&mut profile);
            }
            authorize_spawn_profile_material(
                dsl_authority,
                &entry.agent_identity,
                &entry.role,
                &profile,
                None,
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
            // An explicitly supplied host client remains a mechanical test or
            // embedding override. Otherwise restoration re-enters the
            // canonical factory/provider path; it must never install a
            // long-lived synthetic client as provider identity.
            if let Some(reconcile_client) = default_llm_client.clone() {
                config.llm_client_override = Some(reconcile_client);
            }
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
                session_origin: super::provisioner::ProvisionSessionOrigin::Fresh,
                binding: crate::RuntimeBinding::Session,
                peer_name,
                owner_bridge_session_id: None,
                ops_registry: None,
                generated_self_owned_operation_owner: None,
                runtime_revival_intent: super::provisioner::RuntimeRevivalIntent::None,
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

        // Refresh live endpoint projections before the shared topology repair.
        // Roster replay supplies member metadata, but MobMachine remains the
        // behavior authority for trust repair and pruning.
        let entries = roster.list().cloned().collect::<Vec<_>>();
        let restore_diagnostics_snapshot = tool_handle.restore_diagnostics.read().await.clone();
        seed_mob_authority_restore_failures(dsl_authority, &restore_diagnostics_snapshot)?;
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
                if super::member_runtime_is_host_owned(dsl_authority.state(), &entry.agent_identity)
                {
                    // The member host owns the projected session binding and
                    // its operation-owner context. `active_ids` is local-only.
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
                    provisioner,
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
            if super::member_runtime_is_host_owned(dsl_authority.state(), &entry.agent_identity) {
                // Cross-host recovery owns the exact member endpoint. Never
                // derive it from the controller's local session backend.
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
                let recovered_binding_without_endpoint = entry
                    .member_ref
                    .bridge_session_id()
                    .is_some_and(|bridge_session_id| {
                        new_recovered_session_bindings_without_endpoint
                            .contains(&entry.agent_identity)
                            || legacy_recovered_session_bindings
                                .get(&entry.agent_identity)
                                .is_some_and(|(runtime_id, recovered_session_id)| {
                                    runtime_id == &entry.agent_runtime_id
                                        && recovered_session_id == bridge_session_id
                                })
                    });
                if recovered_binding_without_endpoint {
                    let bridge_session_id =
                        entry.member_ref.bridge_session_id().ok_or_else(|| {
                            MobError::Internal(format!(
                                "recovered endpoint upgrade for '{}' lost its bridge session",
                                entry.agent_identity
                            ))
                        })?;
                    append_recovered_session_binding(
                        dsl_authority,
                        &tool_handle.events,
                        &definition.id,
                        entry,
                        bridge_session_id,
                        Some(spec),
                        "resume_upgrade_recovered_member_peer_endpoint",
                    )
                    .await?;
                } else {
                    register_seeded_member_peer(
                        dsl_authority,
                        &entry.agent_identity,
                        &entry.agent_runtime_id,
                        entry.generation,
                        entry.fence_token,
                        &spec,
                        "resume_register_member_peer",
                    )?;
                }
            } else if let MemberRef::BackendPeer {
                peer_id,
                session_id: None,
                ..
            } = &entry.member_ref
            {
                // New journals already restored the exact backend endpoint
                // from MemberSpawned. Keep that generation-bound descriptor:
                // the peer-only provisioner may project a different display
                // name while reconciling transport, but it is not a second
                // endpoint authority. Legacy journals still fall through and
                // recover from the live backend observation below.
                if dsl_authority
                    .state()
                    .member_peer_endpoints
                    .contains_key(&dsl_identity)
                {
                    continue;
                }
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
                    &entry.agent_runtime_id,
                    entry.generation,
                    entry.fence_token,
                    &spec,
                    "resume_register_backend_member_peer",
                )?;
            }
        }
        reconcile_resume_topology(
            definition,
            roster,
            provisioner,
            &supervisor_bridge,
            &runtime_metadata,
            dsl_authority,
            topology_epoch,
        )
        .await?;
        if notify_orchestrator_on_resume && let Some(orchestrator) = &definition.orchestrator {
            let orchestrator_entries = dsl_authority
                .state()
                .active_member_identities_for_profile(&orchestrator.profile)
                .into_iter()
                .map(|orchestrator_identity| {
                    roster
                        .get(&orchestrator_identity)
                        .cloned()
                        .ok_or_else(|| {
                            MobError::Internal(format!(
                                "active MobMachine orchestrator '{orchestrator_identity}' has no mechanical roster entry during resume"
                            ))
                        })
                })
                .collect::<Result<Vec<_>, MobError>>()?;
            for orchestrator_entry in orchestrator_entries {
                realize_orchestrator_resume_notification(
                    definition,
                    &orchestrator_entry,
                    session_service.as_ref(),
                    provisioner,
                    dsl_authority,
                )
                .await?;
            }
        }
        Ok(())
    }

    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::too_many_arguments)]
    async fn start_runtime(
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
        spawn_base_prompt_source: Option<Arc<dyn super::spec_compiler::SpawnBasePromptSource>>,
        spawn_member_customizer: Option<Arc<dyn super::SpawnMemberCustomizer>>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
        notify_orchestrator_on_resume: bool,
        realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
        controlling_acceptor: Option<ControllingAcceptorConfig>,
        member_live_host: Option<Arc<dyn meerkat_runtime::member_live::MemberLiveHost>>,
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
        let provisioner: Arc<dyn MobProvisioner> = Arc::new(
            MultiBackendProvisioner::new(
                session_service.clone(),
                runtime_adapter.clone(),
                workgraph_service,
                definition.backend.external.clone(),
                Arc::clone(&supervisor_bridge),
            )
            .with_binding_persistence(definition.id.clone(), runtime_metadata.clone()),
        );
        let roster = Arc::new(RwLock::new(RosterAuthority::from_roster(initial_roster)));
        let (command_tx, command_rx) = mpsc::channel(MOB_COMMAND_CHANNEL_CAPACITY);
        let restore_diagnostics = Arc::new(RwLock::new(HashMap::new()));
        let wiring = RuntimeWiring {
            roster,
            dsl_authority,
            machine_state_watch_tx,
            reachability_observations: Arc::new(super::handle::ReachabilityObservations::default()),
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
            command_tx,
            command_rx,
        };

        Self::start_runtime_with_components(
            definition,
            wiring,
            events,
            run_store,
            session_service,
            runtime_adapter,
            provisioner,
            tool_bundles,
            default_llm_client,
            default_external_tools_provider,
            spawn_base_prompt_source,
            spawn_member_customizer,
            realm_profile_store,
            notify_orchestrator_on_resume,
            BTreeMap::new(),
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
            BTreeSet::new(),
            Vec::new(),
            Vec::new(),
            1,
            true,
            realtime_session_factory,
            controlling_acceptor,
            member_live_host,
        )
        .await
    }

    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::too_many_arguments)]
    async fn start_runtime_with_components(
        definition: Arc<MobDefinition>,
        wiring: RuntimeWiring,
        events: Arc<dyn MobEventStore>,
        run_store: Arc<dyn MobRunStore>,
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: RuntimeAdapterOption,
        provisioner: Arc<dyn MobProvisioner>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        default_external_tools_provider: Option<crate::ExternalToolsProvider>,
        spawn_base_prompt_source: Option<Arc<dyn super::spec_compiler::SpawnBasePromptSource>>,
        spawn_member_customizer: Option<Arc<dyn super::SpawnMemberCustomizer>>,
        realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
        notify_orchestrator_on_resume: bool,
        per_spawn_external_tools: BTreeMap<AgentIdentity, Arc<dyn AgentToolDispatcher>>,
        retired_event_index: HashSet<String>,
        retirement_started_event_index: HashSet<String>,
        preserved_respawn_topology_event_index: HashSet<String>,
        recovered_remote_intent_runs: BTreeSet<RunId>,
        recovered_private_remote_turns: Vec<crate::event::RemoteTurnObligationEvent>,
        recovered_host_revocations: Vec<String>,
        recovered_fence_token_floor: u64,
        remote_intent_reconciler_enabled: bool,
        realtime_session_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>>,
        controlling_acceptor: Option<ControllingAcceptorConfig>,
        member_live_host: Option<Arc<dyn meerkat_runtime::member_live::MemberLiveHost>>,
    ) -> Result<MobHandle, MobError> {
        // Recover the actor-local idempotency index from durable public events
        // before spawning any runtime-owned tasks. Private remote-turn custody
        // is then overlaid below from the validated recovery material.
        let durable_events = events.replay_all().await?;
        #[cfg(test)]
        let actor_runtime_id = register_actor_runtime_for_test(&definition.id);
        let next_fence_token = combine_recovered_fence_token_seeds(
            next_fence_token_seed(wiring.dsl_authority.state()),
            recovered_fence_token_floor,
        );
        // Reverse-lane ingress is an explicit host-process composition fact.
        // Never invent a loopback listener here: publishing it to a different
        // machine would make route installation look converged while the peer
        // dials itself. An absent capability fails the mixed-host route closed.
        let run_store = authority_validating_mob_run_store(run_store);
        let RuntimeWiring {
            roster,
            dsl_authority,
            machine_state_watch_tx,
            reachability_observations,
            restore_diagnostics,
            runtime_metadata,
            supervisor_bridge,
            command_tx,
            command_rx,
        } = wiring;
        let handle_session_service = session_service.clone();
        let wiring_public_phase = seeded_mob_public_phase(dsl_authority.state());
        // Terminal-phase watch: seed with the initial phase so a status()
        // call before any DSL transition returns the right answer.
        let (phase_watch_tx_actor, phase_watch_rx) =
            tokio::sync::watch::channel(wiring_public_phase);
        let handle = MobHandle {
            // Explicit launch-site mint (A16): the launching process IS the
            // owning operator — not ambient inference (gotcha #19).
            command_authority: crate::control_policy::CommandAuthority::principal(
                crate::control_policy::MobControlPrincipal::Owner,
            ),
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
            reachability_observations: Arc::clone(&reachability_observations),
            phase_watch_rx,
            realtime_session_factory,
        };
        // Row #320: the orphan budget is MobMachine state (seeded once in
        // `start_runtime` from `definition.limits.max_orphaned_turns`); the
        // executor no longer holds a shell-side budget.
        let actor_flow_executor = ActorFlowTurnExecutor::new(handle.clone(), provisioner.clone());
        // Phase 6 (§7.4): the member event pump manager — one detached
        // `PollMemberEvents` loop per placed member, durable cursors in the
        // runtime metadata store (DEC-P6E-9/10/11).
        let member_event_pumps = Arc::new(super::event_pump::MemberEventPumpManager::new(
            definition.id.clone(),
            supervisor_bridge.clone(),
            runtime_metadata.clone(),
        ));
        member_event_pumps
            .install_reachability_observations(Arc::clone(&reachability_observations));
        // Cross-lane hook wiring (ADJ-P6-10, FLAG-P6F-5): ONE
        // RemoteFlowTicketRegistry per mob, shared by the executor
        // (arm/disarm) and the pump (rows-then-sidecar feed); the A17
        // pump-liveness hook routes through the actor's machine-fact
        // material derivation.
        member_event_pumps.install_outcome_sink(actor_flow_executor.remote_flow_ticket_registry());
        member_event_pumps.install_obligation_probe(Arc::new(
            super::event_pump::HandleObligationProbe {
                handle: handle.clone(),
            },
        ));
        actor_flow_executor.install_remote_turn_obligation_pump(Arc::new(
            super::event_pump::HandleObligationPump {
                handle: handle.clone(),
            },
        ));
        // The actor holds the SAME registry Arc for member-lifetime
        // boundaries (disposal lane drop, placed-respawn rematerializing
        // mark).
        let remote_flow_tickets = actor_flow_executor.remote_flow_ticket_registry();
        let intent_reconciler_provisioner = provisioner.clone();
        let intent_reconciler_tickets = remote_flow_tickets.clone();
        let kickoff_reconciler_provisioner = provisioner.clone();
        let kickoff_reconciler_metadata = runtime_metadata.clone();
        let completion_reconciler_provisioner = provisioner.clone();
        let flow_executor: Arc<dyn FlowTurnExecutor> = Arc::new(actor_flow_executor);
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

        // DEC-U3: the member-operator upcall responder is spawned next to
        // the actor and owned BY the actor (shut down when its run loop
        // exits, Drop-abort backstop). It serves member upcalls arriving on
        // the supervisor bridge runtime's inbox.
        #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
        let upcall_responder = Some(super::upcall_responder::spawn_upcall_responder(
            handle.clone(),
            runtime_metadata.clone(),
        ));

        // Detached host I/O is actor-local, so recovered bound hosts start a
        // fresh volatile binding incarnation. Every later successful
        // bind/rebind/revoke boundary advances this fence and purges release
        // reservations minted by its predecessor.
        let host_binding_incarnations = dsl_authority
            .state()
            .host_bind_phase
            .iter()
            .filter(|(_, phase)| **phase == mob_dsl::HostBindPhase::Bound)
            .map(|(host_id, _)| (host_id.clone(), 1_u64))
            .collect();
        let placed_completion_durable_index = Arc::new(std::sync::Mutex::new(
            super::actor::PlacedCompletionDurableIndex::recover_with_private_remote_turns(
                &durable_events,
                &definition.id,
                &recovered_private_remote_turns,
            )?,
        ));

        let mut actor = MobActor {
            definition,
            roster,
            events,
            placed_completion_durable_index,
            run_store,
            provisioner,
            flow_engine,
            has_orchestrator,
            notify_orchestrator_on_resume,
            run_tasks: BTreeMap::new(),
            run_cancel_tokens: BTreeMap::new(),
            flow_streams: handle.flow_streams.clone(),
            command_tx,
            tool_bundles,
            default_llm_client,
            retired_event_index: Arc::new(RwLock::new(retired_event_index)),
            retirement_started_event_index: Arc::new(RwLock::new(retirement_started_event_index)),
            preserved_respawn_topology_event_index: Arc::new(RwLock::new(
                preserved_respawn_topology_event_index,
            )),
            autonomous_initial_turns: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            autonomous_stop_interrupts: BTreeMap::new(),
            autonomous_stop_interrupted: BTreeMap::new(),
            autonomous_stop_interrupt_cursor: 0,
            next_spawn_ticket: 0,
            // ADJ-8 (multi-host fence reseed): the shell fence counter must
            // sort ABOVE every replayed fence after a controlling restart —
            // the member-host dedup ladder is ORDER-comparing (StaleFence on
            // a tuple below the recorded row), so a reset-to-1 counter would
            // draw spurious permanent rejects for legitimate re-issues. The
            // machine still owns every fence guard; this is a mechanical
            // monotonicity seed over both recovered machine maxima and every
            // canonical placed carrier loaded at startup. Pending/terminal
            // carriers may already have been exactly cleaned, so the explicit
            // floor preserves their fence high-water after their machine maps
            // are intentionally resolved.
            next_fence_token: std::sync::atomic::AtomicU64::new(next_fence_token),
            pending_spawns: PendingSpawnLineage::new(),
            pending_spawn_cleanup_anchors: BTreeMap::new(),
            edge_locks: Arc::new(super::edge_locks::EdgeLockRegistry::new()),
            lifecycle_tasks: tokio::task::JoinSet::new(),
            actor_io_tasks: tokio::task::JoinSet::new(),
            member_live_mutation_tasks: tokio::task::JoinSet::new(),
            member_live_open_cleanup_obligations: BTreeMap::new(),
            member_live_open_cleanup_inflight: BTreeSet::new(),
            next_member_live_open_cleanup_ticket: 0,
            orphan_release_reservations: BTreeMap::new(),
            host_binding_incarnations,
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
            member_live_host,
            member_event_pumps,
            remote_flow_tickets,
            reachability_observations,
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            upcall_responder,
            spawn_policy,
            dsl_authority: *dsl_authority,
            dsl_topology_epoch,
            dsl_authority_owner_token,
            machine_state_watch_tx,
            phase_watch_tx: phase_watch_tx_actor,
            default_external_tools_provider,
            per_spawn_external_tools: tokio::sync::RwLock::new(per_spawn_external_tools),
            spawn_base_prompt_source,
            spawn_member_customizer,
            realm_profile_store,
            composition_binding,
            pending_routed_effects: Vec::new(),
            destroy_cleanup_active: false,
            durable_uncertainty_fail_stop: false,
            respawn_topology_reply_withheld: false,
            #[cfg(not(target_arch = "wasm32"))]
            controlling_acceptor: controlling_acceptor
                .map(super::actor::ControllingAcceptorState::new),
        };
        #[cfg(target_arch = "wasm32")]
        let _ = controlling_acceptor;
        // A17 recovery trigger (DEC-P6E-11): obligations survive via event
        // replay, subscriptions do not — re-derive pending remote-turn
        // obligations from recovered machine state and keep their members'
        // pumps alive so recovered flow terminals still drain.
        let recovered_obligations: Vec<crate::ids::AgentIdentity> = actor
            .dsl_authority
            .state()
            .pending_remote_turn_outcomes
            .iter()
            .chain(
                actor
                    .dsl_authority
                    .state()
                    .committed_remote_turn_outcomes
                    .iter(),
            )
            .chain(
                actor
                    .dsl_authority
                    .state()
                    .resolved_remote_turn_outcomes
                    .iter(),
            )
            .map(|obligation| crate::ids::AgentIdentity::from(obligation.agent_identity.0.as_str()))
            .chain(
                actor
                    .dsl_authority
                    .state()
                    .pending_placed_kickoff_outcomes
                    .iter()
                    .chain(
                        actor
                            .dsl_authority
                            .state()
                            .resolved_placed_kickoff_outcomes
                            .iter(),
                    )
                    .map(|obligation| {
                        crate::ids::AgentIdentity::from(obligation.agent_identity.0.as_str())
                    }),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        // A durable MobDestroying marker is a retry anchor, not a completed
        // destroy. Public authority is already closed by destroy_admitted,
        // but the recovered actor must automatically resume the exact member,
        // host, and storage cleanup rather than waiting for an operator verb.
        let resume_destroy_cleanup = actor.dsl_authority.state().destroy_admitted
            && actor.dsl_authority.state().lifecycle_phase
                != crate::machines::mob_machine::MobPhase::Destroyed;
        let recovered_lifecycle_retry_intent = match (
            actor.dsl_authority.state().lifecycle_phase,
            actor
                .dsl_authority
                .state()
                .placed_completion_lifecycle_intent,
        ) {
            (
                crate::machines::mob_machine::MobPhase::Stopped,
                Some(mob_dsl::PlacedCompletionLifecycleIntentKind::Stop),
            )
            | (
                crate::machines::mob_machine::MobPhase::Completed,
                Some(mob_dsl::PlacedCompletionLifecycleIntentKind::Complete),
            )
            | (crate::machines::mob_machine::MobPhase::Destroyed, _) => None,
            (_, intent) => intent,
        };
        let recovered_retirements = if resume_destroy_cleanup {
            Vec::new()
        } else {
            actor
                .dsl_authority
                .state()
                .identity_to_runtime
                .iter()
                .filter(|(_, runtime_id)| {
                    matches!(
                        actor
                            .dsl_authority
                            .state()
                            .member_state_markers
                            .get(*runtime_id),
                        Some(crate::machines::mob_machine::MobMemberState::Retiring)
                    )
                })
                .map(|(identity, _)| crate::ids::AgentIdentity::from(identity.0.as_str()))
                .collect::<Vec<_>>()
        };
        // Cold-recovery convergence workers are owned by this actor instance.
        // Register them before moving the actor into its run loop so every
        // terminal path can abort and join them before command-channel closure.
        if !recovered_obligations.is_empty() {
            let pump_handle = handle.clone();
            actor.actor_io_tasks.spawn(async move {
                #[cfg(test)]
                let _startup_worker_guard = ActorOwnedStartupWorkerGuard::new(actor_runtime_id);
                for identity in recovered_obligations {
                    if let Err(error) = pump_handle.ensure_pump_for_obligation(&identity).await {
                        tracing::warn!(
                            agent_identity = %identity,
                            error = %error,
                            "recovered remote-turn obligation could not keep its pump alive"
                        );
                    }
                }
            });
        }
        if !resume_destroy_cleanup {
            for host_id in recovered_host_revocations {
                let revoke_handle = handle.clone();
                actor.actor_io_tasks.spawn(async move {
                    #[cfg(test)]
                    let _startup_worker_guard = ActorOwnedStartupWorkerGuard::new(actor_runtime_id);
                    let mut retry_delay = std::time::Duration::from_millis(25);
                    loop {
                        match revoke_handle.revoke_host(&host_id).await {
                            Ok(_) => break,
                            Err(MobError::ActorCommandChannelClosed) => break,
                            Err(error) => {
                                tracing::warn!(
                                    mob_id = %revoke_handle.mob_id(),
                                    host_id = %host_id,
                                    error = %error,
                                    retry_delay_ms = retry_delay.as_millis(),
                                    "automatic recovery of durable host revoke remains incomplete"
                                );
                                tokio::time::sleep(retry_delay).await;
                                retry_delay = retry_delay
                                    .saturating_mul(2)
                                    .min(std::time::Duration::from_secs(2));
                            }
                        }
                    }
                });
            }
        }
        for identity in recovered_retirements {
            let retire_handle = handle.clone();
            actor.actor_io_tasks.spawn(async move {
                #[cfg(test)]
                let _startup_worker_guard = ActorOwnedStartupWorkerGuard::new(actor_runtime_id);
                let mut retry_delay = std::time::Duration::from_millis(25);
                loop {
                    match retire_handle.retire(identity.clone()).await {
                        Ok(()) | Err(MobError::MemberNotFound(_)) => break,
                        Err(MobError::ActorCommandChannelClosed) => break,
                        Err(error) => {
                            tracing::warn!(
                                mob_id = %retire_handle.mob_id(),
                                agent_identity = %identity,
                                error = %error,
                                retry_delay_ms = retry_delay.as_millis(),
                                "automatic recovery of durable member retirement remains incomplete"
                            );
                            tokio::time::sleep(retry_delay).await;
                            retry_delay = retry_delay
                                .saturating_mul(2)
                                .min(std::time::Duration::from_secs(2));
                        }
                    }
                }
            });
        }
        if resume_destroy_cleanup {
            let destroy_handle = handle.clone();
            actor.actor_io_tasks.spawn(async move {
                #[cfg(test)]
                let _startup_worker_guard = ActorOwnedStartupWorkerGuard::new(actor_runtime_id);
                let mut retry_delay = std::time::Duration::from_millis(25);
                loop {
                    match destroy_handle.destroy().await {
                        Ok(_) => break,
                        Err(super::handle::MobDestroyError::Mob(
                            MobError::ActorCommandChannelClosed,
                        )) => break,
                        Err(error) => {
                            tracing::warn!(
                                mob_id = %destroy_handle.mob_id(),
                                error = %error,
                                retry_delay_ms = retry_delay.as_millis(),
                                "automatic recovery of admitted destroy cleanup remains incomplete"
                            );
                            tokio::time::sleep(retry_delay).await;
                            retry_delay = retry_delay
                                .saturating_mul(2)
                                .min(std::time::Duration::from_secs(2));
                        }
                    }
                }
            });
        } else if let Some(intent) = recovered_lifecycle_retry_intent {
            let lifecycle_handle = handle.clone();
            actor.actor_io_tasks.spawn(async move {
                #[cfg(test)]
                let _startup_worker_guard = ActorOwnedStartupWorkerGuard::new(actor_runtime_id);
                drive_recovered_placed_lifecycle_intent(lifecycle_handle, intent).await;
            });
        }
        if remote_intent_reconciler_enabled {
            let completion_reconciler_task =
                super::placed_completion_reconciler::PlacedCompletionReconciler::run_owned(
                    handle.mob_id().clone(),
                    handle.clone(),
                    completion_reconciler_provisioner,
                );
            actor.actor_io_tasks.spawn(async move {
                #[cfg(test)]
                let _startup_worker_guard = ActorOwnedStartupWorkerGuard::new(actor_runtime_id);
                completion_reconciler_task.await;
            });
            let kickoff_reconciler_task =
                super::placed_kickoff_reconciler::PlacedKickoffReconciler::run_owned(
                    handle.mob_id().clone(),
                    handle.clone(),
                    kickoff_reconciler_provisioner,
                    kickoff_reconciler_metadata,
                );
            actor.actor_io_tasks.spawn(async move {
                #[cfg(test)]
                let _startup_worker_guard = ActorOwnedStartupWorkerGuard::new(actor_runtime_id);
                kickoff_reconciler_task.await;
            });
            let reconciler_task =
                super::remote_turn_reconciler::RemoteTurnIntentReconciler::run_owned(
                    handle.mob_id().clone(),
                    handle.clone(),
                    intent_reconciler_provisioner,
                    intent_reconciler_tickets,
                    recovered_remote_intent_runs,
                );
            actor.actor_io_tasks.spawn(async move {
                #[cfg(test)]
                let _startup_worker_guard = ActorOwnedStartupWorkerGuard::new(actor_runtime_id);
                reconciler_task.await;
            });
        }
        tokio::spawn(actor.run(command_rx));

        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{MemberSpawnedEvent, MobEvent, MobEventKind};
    use crate::ids::{Generation, StepId};
    use crate::machines::mob_machine as mob_dsl;
    use crate::roster::{MobMemberKickoffPhase, MobMemberKickoffSnapshot};
    use crate::store::{
        InMemoryMobEventStore, InMemoryMobRuntimeMetadataStore, MobEventStore,
        MobRuntimeMetadataStore,
    };
    use chrono::Utc;
    use meerkat_core::time_compat::UNIX_EPOCH;

    #[cfg(not(target_arch = "wasm32"))]
    struct NoAcceptorMaterial;

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait::async_trait]
    impl LocalMemberAcceptorMaterialSource for NoAcceptorMaterial {
        async fn registration_for(
            &self,
            _session_id: &SessionId,
        ) -> Option<MemberAcceptorRegistration> {
            None
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn acceptor_registration(keypair: Arc<meerkat_comms::Keypair>) -> MemberAcceptorRegistration {
        let (_inbox, inbox_sender) = meerkat_comms::Inbox::new();
        MemberAcceptorRegistration {
            pubkey: keypair.public_key(),
            keypair,
            inbox_sender,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn process_acceptor_is_shared_and_registration_cleanup_is_exact() {
        let config = ControllingAcceptorConfig::new(
            "127.0.0.1:0".parse().expect("loopback address"),
            None,
            Arc::new(NoAcceptorMaterial),
        );
        let sibling = config.clone();
        assert!(Arc::ptr_eq(&config.shared, &sibling.shared));

        let durable_key = Arc::new(meerkat_comms::Keypair::generate());
        let old = config
            .register(
                "mob-a/member/session".to_string(),
                acceptor_registration(Arc::clone(&durable_key)),
            )
            .await
            .expect("first registration");
        let replacement = sibling
            .register(
                "mob-a/member/session".to_string(),
                acceptor_registration(Arc::clone(&durable_key)),
            )
            .await
            .expect("same logical owner replaces its inbox");
        assert_eq!(old.advertised_address, replacement.advertised_address);

        config.remove(&old).await.expect("stale cleanup is a no-op");
        {
            let live = config.shared.live.lock().await;
            let current = live
                .as_ref()
                .expect("shared listener remains live")
                .leases
                .get(&replacement.key)
                .expect("replacement registration remains");
            assert!(Arc::ptr_eq(&current.token, &replacement.token));
        }

        let conflict = config
            .register(
                "mob-b/other/session".to_string(),
                acceptor_registration(durable_key),
            )
            .await
            .expect_err("a different logical owner cannot claim the same key");
        assert!(conflict.to_string().contains("different mob member"));

        config
            .remove(&replacement)
            .await
            .expect("exact replacement cleanup");
        let live = config.shared.live.lock().await;
        assert!(
            !live
                .as_ref()
                .expect("listener remains process-owned")
                .leases
                .contains_key(&replacement.key)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn actor_registration_refreshes_same_durable_key_to_current_inbox() {
        let config = ControllingAcceptorConfig::new(
            "127.0.0.1:0".parse().expect("loopback address"),
            None,
            Arc::new(NoAcceptorMaterial),
        );
        let mut actor_state = super::actor::ControllingAcceptorState::new(config.clone());
        let durable_key = Arc::new(meerkat_comms::Keypair::generate());
        let key = durable_key.public_key().to_pubkey_string();

        actor_state
            .refresh_registration(
                "mob-a/member/session".to_string(),
                acceptor_registration(Arc::clone(&durable_key)),
            )
            .await
            .expect("initial actor publication");
        let first = actor_state
            .registration_token(&key)
            .expect("first actor lease");

        actor_state
            .refresh_registration(
                "mob-a/member/session".to_string(),
                acceptor_registration(durable_key),
            )
            .await
            .expect("revived inbox publication");
        let current = actor_state
            .registration_token(&key)
            .expect("replacement actor lease");
        assert!(
            !Arc::ptr_eq(&first, &current),
            "same durable key must mint a fresh lease for the revived inbox"
        );
        let live = config.shared.live.lock().await;
        let shared = live
            .as_ref()
            .expect("shared listener")
            .leases
            .get(&key)
            .expect("shared current lease");
        assert!(Arc::ptr_eq(&shared.token, &current));
        drop(live);

        actor_state.shutdown().await;
        let live = config.shared.live.lock().await;
        assert!(
            !live
                .as_ref()
                .expect("listener remains process-owned")
                .leases
                .contains_key(&key)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn actor_registration_rotation_removes_old_key_and_preserves_replacement() {
        let config = ControllingAcceptorConfig::new(
            "127.0.0.1:0".parse().expect("loopback address"),
            None,
            Arc::new(NoAcceptorMaterial),
        );
        let mut actor_state = super::actor::ControllingAcceptorState::new(config.clone());
        let old_key = Arc::new(meerkat_comms::Keypair::generate());
        let replacement_key = Arc::new(meerkat_comms::Keypair::generate());
        let old_pubkey = old_key.public_key();
        let replacement_pubkey = replacement_key.public_key();
        let old_key_string = old_pubkey.to_pubkey_string();
        let replacement_key_string = replacement_pubkey.to_pubkey_string();

        actor_state
            .refresh_registration(
                "mob-a/member/old-session".to_string(),
                acceptor_registration(old_key),
            )
            .await
            .expect("old incarnation publication");
        actor_state
            .remove_registration(&old_pubkey)
            .await
            .expect("old incarnation cleanup");
        actor_state
            .refresh_registration(
                "mob-a/member/replacement-session".to_string(),
                acceptor_registration(replacement_key),
            )
            .await
            .expect("replacement incarnation publication");
        let replacement_token = actor_state
            .registration_token(&replacement_key_string)
            .expect("replacement actor lease");

        actor_state
            .remove_registration(&old_pubkey)
            .await
            .expect("repeated old cleanup is a no-op");
        let live = config.shared.live.lock().await;
        let leases = &live.as_ref().expect("shared listener remains live").leases;
        assert!(!leases.contains_key(&old_key_string));
        let replacement = leases
            .get(&replacement_key_string)
            .expect("replacement registration remains");
        assert!(Arc::ptr_eq(&replacement.token, &replacement_token));
        drop(live);

        actor_state.shutdown().await;
    }

    #[cfg(feature = "runtime-adapter")]
    #[test]
    fn placed_orchestrator_resume_notification_remote_failures_are_non_gating() {
        let member_id = AgentIdentity::from("remote-orchestrator");
        let non_gating = [
            MobError::CommsError(SendError::PeerNotFound("remote-orchestrator".to_string())),
            MobError::CommsError(SendError::PeerOffline),
            MobError::CommsError(SendError::Transport("connection reset".to_string())),
            MobError::BridgeRequestTimedOut {
                request_envelope_id: "resume-notification".to_string(),
                timeout_ms: 5_000,
            },
            MobError::BridgeCommandRejected {
                cause: super::super::bridge_protocol::BridgeRejectionCause::StaleFence,
                reason: "controller has not re-probed the restarted host yet".to_string(),
            },
            MobError::BridgeDeliveryRejected {
                cause: Box::new(
                    super::super::bridge_protocol::BridgeDeliveryRejectionCause::Internal {
                        detail: "remote runtime refused an informational turn".to_string(),
                    },
                ),
                reason: "remote terminal response".to_string(),
            },
        ];
        for error in non_gating {
            assert!(
                settle_placed_orchestrator_resume_notification(&member_id, Err(error)).is_ok(),
                "typed remote notification unavailability must not abort cold resume"
            );
        }
    }

    #[cfg(feature = "runtime-adapter")]
    #[test]
    fn placed_orchestrator_resume_notification_keeps_authority_failures_gating() {
        let member_id = AgentIdentity::from("remote-orchestrator");
        let gating = [
            MobError::Internal("malformed recovered placement".to_string()),
            MobError::StorageError(Box::new(std::io::Error::other(
                "persisted authority could not be read",
            ))),
            MobError::ExternalMemberCleanupUncertain {
                reason: "recipient trust rollback was not certified".to_string(),
            },
            MobError::SupervisorRotationIncomplete {
                previous_epoch: 1,
                attempted_epoch: 2,
                attempted_public_peer_id: "replacement-supervisor".to_string(),
                rotated_peer_count: 1,
                rollback_succeeded: false,
                pending_authority_recorded: true,
                rollback_error: None,
                reason: "rotation is pending".to_string(),
            },
            MobError::CommsError(SendError::Internal(
                "local comms invariant failure".to_string(),
            )),
            MobError::CommsError(SendError::AdmissionDropped {
                reason: meerkat_core::comms::AdmissionDropReason::UntrustedSender,
            }),
        ];
        for error in gating {
            assert!(
                settle_placed_orchestrator_resume_notification(&member_id, Err(error)).is_err(),
                "authority, cleanup, and invariant failures must still abort cold resume"
            );
        }
    }

    #[cfg(feature = "runtime-adapter")]
    #[test]
    fn placed_kickoff_cold_recovery_uses_scoped_turn_keys() {
        let input_id = uuid::Uuid::new_v4().to_string();
        let base = crate::event::PlacedKickoffObligationEvent {
            agent_identity: AgentIdentity::from("worker-a"),
            host_id: "host-a".to_string(),
            host_binding_generation: 3,
            member_session_id: "session-a".to_string(),
            generation: Generation::new(2),
            fence_token: FenceToken::new(7),
            input_id: input_id.clone(),
            objective_id: meerkat_core::interaction::ObjectiveId::new(),
        };
        let mut other_scope = base.clone();
        other_scope.agent_identity = AgentIdentity::from("worker-b");
        assert_ne!(
            PlacedKickoffTurnKey::from(&base),
            PlacedKickoffTurnKey::from(&other_scope),
            "the same UUID in another member/host/incarnation scope is unrelated"
        );

        let mut conflicting_custody = base.clone();
        conflicting_custody.objective_id = meerkat_core::interaction::ObjectiveId::new();
        assert_eq!(
            PlacedKickoffTurnKey::from(&base),
            PlacedKickoffTurnKey::from(&conflicting_custody),
            "objective/session/binding differences remain corruption within one TurnKey"
        );
        assert_ne!(
            PlacedKickoffRecoveryKey::from_event(&base),
            PlacedKickoffRecoveryKey::from_event(&conflicting_custody)
        );
    }

    fn placed_spawned_event(
        mob_id: &MobId,
        cursor: u64,
        identity: &AgentIdentity,
        generation: Generation,
        fence_token: FenceToken,
        placed_spawn_id: &crate::ids::PlacedSpawnId,
    ) -> MobEvent {
        let mut spawned = MemberSpawnedEvent::new(
            identity.clone(),
            generation,
            fence_token,
            AgentRuntimeId::new(identity.clone(), generation),
            ProfileName::from("worker"),
        );
        spawned.runtime_mode = crate::MobRuntimeMode::TurnDriven;
        spawned = spawned
            .with_bridge_member_ref(Some(crate::event::MemberRef::from_bridge_session_id(
                meerkat_core::SessionId::new(),
            )))
            .with_placed_spawn_id(Some(placed_spawn_id.clone()));
        MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: mob_id.clone(),
            kind: MobEventKind::MemberSpawned(spawned),
        }
    }

    fn test_portable_member_spec(
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> meerkat_contracts::wire::PortableMemberSpec {
        use meerkat_contracts::wire::{
            PortableDefinitionExtract, PortableMemberSpec, PortableProfile, PortableSpawnOverlay,
            PortableSystemPrompt, PortableToolConfig, WireMobRuntimeMode,
            WireSpawnContinuityIntent,
        };

        PortableMemberSpec {
            mob_id: mob_id.as_str().to_string(),
            profile_name: "worker".to_string(),
            agent_identity: identity.as_str().to_string(),
            profile: PortableProfile {
                model: "test-model".to_string(),
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: PortableToolConfig::default(),
                peer_description: String::new(),
                external_addressable: true,
                runtime_mode: WireMobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
            definition_extract: PortableDefinitionExtract {
                profile_names: vec!["worker".to_string()],
                ..PortableDefinitionExtract::default()
            },
            overlay: PortableSpawnOverlay {
                context: None,
                labels: None,
                additional_instructions: None,
                system_prompt: PortableSystemPrompt::Disable,
                tool_access_policy: None,
                mob_tool_authority_context: None,
                auth_binding: None,
                budget_limits: None,
                runtime_mode: WireMobRuntimeMode::TurnDriven,
                continuity_intent: WireSpawnContinuityIntent::default(),
            },
            required_env_keys: Vec::new(),
        }
    }

    fn test_placed_carrier(
        mob_id: &MobId,
        identity: &AgentIdentity,
        generation: Generation,
        fence_token: FenceToken,
        spawn_id: crate::ids::PlacedSpawnId,
        committed: bool,
    ) -> crate::store::MobPlacedSpawnCarrierRecord {
        let host_id = meerkat_core::comms::PeerId::from_ed25519_pubkey(&[9; 32]);
        let spec = test_portable_member_spec(mob_id, identity);
        let spec_digest = meerkat_contracts::wire::portable_member_spec_digest(&spec)
            .expect("canonical portable spec digest");
        let mut record = crate::store::MobPlacedSpawnCarrierRecord::pending(
            spawn_id,
            identity.as_str().to_string(),
            generation.get(),
            fence_token.get(),
            meerkat_core::ops::OperationId::new(),
            meerkat_core::SessionId::new(),
            host_id,
            1,
            spec_digest,
            spec,
            None,
            false,
            false,
        );
        if committed {
            let signing_key = [3; 32];
            let peer_id =
                meerkat_core::comms::PeerId::from_ed25519_pubkey(&signing_key).to_string();
            let name =
                meerkat_core::MemberCommsName::new(mob_id.as_str(), "worker", identity.as_str())
                    .expect("canonical member comms name")
                    .to_string();
            record.phase = crate::store::PlacedSpawnCarrierPhase::Committed(
                crate::store::PlacedSpawnCommitRecord {
                    member_session_id: meerkat_core::SessionId::new(),
                    member_peer_endpoint: mob_dsl::MemberPeerEndpoint {
                        name: mob_dsl::PeerName(name),
                        peer_id: mob_dsl::PeerId(peer_id),
                        address: mob_dsl::PeerAddress("tcp://127.0.0.1:4711".to_string()),
                        signing_key: mob_dsl::PeerSigningKey(signing_key),
                    },
                    ack_engine_version: "placed-recovery-test-engine".to_string(),
                },
            );
        }
        record
            .validate_for_mob(mob_id)
            .expect("valid test placed carrier");
        record
    }

    fn test_autonomous_placed_carrier(
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> crate::store::MobPlacedSpawnCarrierRecord {
        let mut record = test_placed_carrier(
            mob_id,
            identity,
            Generation::INITIAL,
            FenceToken::new(73),
            crate::ids::PlacedSpawnId::new(),
            true,
        );
        record.spec.profile.runtime_mode =
            meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost;
        record.spec.overlay.runtime_mode =
            meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost;
        record.kickoff_intent = Some(crate::store::MobPlacedKickoffIntent {
            input_id: uuid::Uuid::new_v4().to_string(),
            objective_id: meerkat_core::interaction::ObjectiveId::new(),
            prompt: meerkat_core::types::ContentInput::Text("durable kickoff".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            injected_context: Vec::new(),
        });
        record.spec_digest = meerkat_contracts::wire::portable_member_spec_digest(&record.spec)
            .expect("autonomous carrier digest");
        record
            .validate_for_mob(mob_id)
            .expect("valid autonomous placed carrier");
        record
    }

    fn test_current_carrier_plan(
        record: &crate::store::MobPlacedSpawnCarrierRecord,
    ) -> PlacedCarrierRecoveryPlan {
        PlacedCarrierRecoveryPlan {
            entries: vec![PlacedCarrierRecoveryEntry {
                record: record.clone(),
                disposition: PlacedCarrierRecoveryDisposition::CommittedCurrent,
            }],
            current_committed_event_keys: BTreeSet::from([(
                record.agent_identity.clone(),
                record.generation,
                record.spawn_id.clone(),
            )]),
            next_carrier_fence_token: record.fence_token + 1,
        }
    }

    fn test_authority_for_current_carrier(
        record: &crate::store::MobPlacedSpawnCarrierRecord,
        plan: &PlacedCarrierRecoveryPlan,
    ) -> mob_dsl::MobMachineAuthority {
        test_authority_for_current_carrier_at_host_generation(
            record,
            plan,
            record.host_binding_generation,
        )
    }

    fn test_authority_for_current_carrier_at_host_generation(
        record: &crate::store::MobPlacedSpawnCarrierRecord,
        plan: &PlacedCarrierRecoveryPlan,
        current_host_binding_generation: u64,
    ) -> mob_dsl::MobMachineAuthority {
        let mut authority = seed_mob_authority();
        authority
            .apply_signal(mob_dsl::MobMachineSignal::RecoverOwnerBridgeSession {
                bridge_session_id: mob_dsl::SessionId(
                    record.operation_owner_session_id.to_string(),
                ),
                destroy_on_owner_archive: false,
                implicit_delegation_mob: false,
            })
            .expect("recover exact carrier owner");
        authority
            .apply_signal(mob_dsl::MobMachineSignal::RecoverHostBinding {
                host_id: mob_dsl::HostId(record.host_id.to_string()),
                pubkey: mob_dsl::PeerSigningKey([9; 32]),
                endpoint: mob_dsl::PeerAddress("tcp://host/kickoff-recovery".to_string()),
                epoch: 1,
                binding_generation: current_host_binding_generation,
                protocol_min: 4,
                protocol_max: 4,
                engine_version: "kickoff-recovery-test".to_string(),
                durable_sessions: true,
                autonomous_members: true,
                hard_cancel_member: true,
                tracked_input_cancel: true,
                memory_store: false,
                mcp: false,
                resolvable_providers: BTreeSet::new(),
                approval_forwarding: false,
                live_endpoint: None,
            })
            .expect("recover exact carrier host");
        recover_current_committed_placed_carrier_signals(&mut authority, plan)
            .expect("recover current autonomous carrier");
        authority
    }

    fn exact_placed_spawned_event(
        mob_id: &MobId,
        cursor: u64,
        record: &crate::store::MobPlacedSpawnCarrierRecord,
    ) -> MobEvent {
        let repaired = repaired_placed_spawn_event(mob_id, record)
            .expect("committed carrier projects an exact spawn event");
        MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: mob_id.clone(),
            kind: repaired.kind,
        }
    }

    fn retired_event(
        mob_id: &MobId,
        cursor: u64,
        identity: &AgentIdentity,
        generation: Generation,
    ) -> MobEvent {
        MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: mob_id.clone(),
            kind: MobEventKind::MemberRetired {
                agent_identity: identity.clone(),
                generation,
                role: ProfileName::from("worker"),
            },
        }
    }

    fn test_placed_kickoff_obligation(
        identity: &AgentIdentity,
    ) -> crate::event::PlacedKickoffObligationEvent {
        crate::event::PlacedKickoffObligationEvent {
            agent_identity: identity.clone(),
            host_id: meerkat_core::comms::PeerId::from_ed25519_pubkey(&[31; 32]).to_string(),
            host_binding_generation: 4,
            member_session_id: meerkat_core::SessionId::new().to_string(),
            generation: Generation::new(2),
            fence_token: FenceToken::new(7),
            input_id: uuid::Uuid::new_v4().to_string(),
            objective_id: meerkat_core::interaction::ObjectiveId::new(),
        }
    }

    fn test_event(mob_id: &MobId, cursor: u64, kind: MobEventKind) -> MobEvent {
        MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: mob_id.clone(),
            kind,
        }
    }

    fn test_remote_turn_obligation() -> crate::event::RemoteTurnObligationEvent {
        crate::event::RemoteTurnObligationEvent {
            agent_identity: AgentIdentity::from("placed-worker"),
            host_id: "host-a".to_string(),
            host_binding_generation: 3,
            member_session_id: "session-a".to_string(),
            generation: Generation::new(2),
            fence_token: FenceToken::new(7),
            dispatch_sequence: 11,
            input_id: uuid::Uuid::new_v4().to_string(),
            run_id: RunId::new(),
            step_id: StepId::from("remote-step"),
        }
    }

    #[test]
    fn finalized_remote_turn_recovery_does_not_require_privacy_rows() {
        let mob_id = MobId::from("remote-turn-finalized-recovery");
        let obligation = test_remote_turn_obligation();
        let target = AgentRuntimeId::new(obligation.agent_identity.clone(), obligation.generation);
        let events = vec![
            test_event(
                &mob_id,
                1,
                MobEventKind::RemoteTurnObligationRecorded {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                2,
                MobEventKind::StepTargetCompleted {
                    run_id: obligation.run_id.clone(),
                    step_id: obligation.step_id.clone(),
                    target,
                    output: Some(serde_json::json!({"answer": 42})),
                    remote_turn_obligation: Some(obligation.clone()),
                },
            ),
            test_event(
                &mob_id,
                3,
                MobEventKind::RemoteTurnOutcomeResolved {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                4,
                MobEventKind::RemoteTurnOutcomeAcknowledged { obligation },
            ),
        ];

        recover_remote_turn_outcome_custody(&mut seed_mob_authority(), &events, &[], &[])
            .expect("ACK is the durable privacy-cleanup proof after intent/receipt deletion");
    }

    #[test]
    fn nonfinal_remote_turn_terminal_still_requires_private_receipt() {
        let mob_id = MobId::from("remote-turn-unfinalized-recovery");
        let obligation = test_remote_turn_obligation();
        let target = AgentRuntimeId::new(obligation.agent_identity.clone(), obligation.generation);
        let events = vec![
            test_event(
                &mob_id,
                1,
                MobEventKind::RemoteTurnObligationRecorded {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                2,
                MobEventKind::StepTargetCompleted {
                    run_id: obligation.run_id.clone(),
                    step_id: obligation.step_id.clone(),
                    target,
                    output: Some(serde_json::json!({"answer": 42})),
                    remote_turn_obligation: Some(obligation),
                },
            ),
        ];

        let error =
            recover_remote_turn_outcome_custody(&mut seed_mob_authority(), &events, &[], &[])
                .expect_err("a nonfinal terminal without its private receipt must fail closed");
        assert!(error.to_string().contains("without its durable receipt"));
    }

    #[test]
    fn remote_turn_dispose_without_terminal_carrier_fails_closed() {
        let mob_id = MobId::from("remote-turn-impossible-dispose-chain");
        let obligation = test_remote_turn_obligation();
        let events = vec![
            test_event(
                &mob_id,
                1,
                MobEventKind::RemoteTurnObligationRecorded {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                2,
                MobEventKind::RemoteTurnOutcomeDisposed { obligation },
            ),
        ];

        let error =
            recover_remote_turn_outcome_custody(&mut seed_mob_authority(), &events, &[], &[])
                .expect_err("Record + Dispose without a terminal is not a valid public chain");
        assert!(error.to_string().contains("without its terminal carrier"));
    }

    #[test]
    fn remote_turn_ack_and_dispose_finalizers_are_mutually_exclusive() {
        let mob_id = MobId::from("remote-turn-conflicting-finalizers");
        let obligation = test_remote_turn_obligation();
        let target = AgentRuntimeId::new(obligation.agent_identity.clone(), obligation.generation);
        let events = vec![
            test_event(
                &mob_id,
                1,
                MobEventKind::RemoteTurnObligationRecorded {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                2,
                MobEventKind::StepTargetCompleted {
                    run_id: obligation.run_id.clone(),
                    step_id: obligation.step_id.clone(),
                    target,
                    output: Some(serde_json::json!({"answer": 42})),
                    remote_turn_obligation: Some(obligation.clone()),
                },
            ),
            test_event(
                &mob_id,
                3,
                MobEventKind::RemoteTurnOutcomeAcknowledged {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                4,
                MobEventKind::RemoteTurnOutcomeDisposed { obligation },
            ),
        ];

        let error =
            recover_remote_turn_outcome_custody(&mut seed_mob_authority(), &events, &[], &[])
                .expect_err("one custody chain cannot have both public finalizers");
        assert!(
            error
                .to_string()
                .contains("mutually exclusive ACK and Dispose")
        );
    }

    #[cfg(feature = "runtime-adapter")]
    #[test]
    fn placed_completion_close_requires_cancellation_predecessor() {
        let mob_id = MobId::from("placed-completion-close-order");
        let obligation = crate::event::PlacedCompletionObligationEvent {
            agent_identity: AgentIdentity::from("placed-worker"),
            host_id: "host-a".to_string(),
            host_binding_generation: 3,
            member_session_id: "session-a".to_string(),
            generation: Generation::new(2),
            fence_token: FenceToken::new(7),
            dispatch_sequence: 17,
            input_id: uuid::Uuid::new_v4().to_string(),
        };
        let events = vec![
            test_event(
                &mob_id,
                1,
                MobEventKind::PlacedCompletionObligationRecorded {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                2,
                MobEventKind::PlacedCompletionOutcomeClosed {
                    obligation,
                    closure: crate::event::PlacedCompletionClosureEvent::HostNoEffect,
                },
            ),
        ];

        let error = collect_placed_completion_recovery_chains(&events)
            .expect_err("Close without RequestCancellation must fail cold recovery");
        assert!(error.to_string().contains("cancellation predecessor"));
    }

    #[test]
    fn placed_kickoff_recovery_rejects_ack_before_resolved_terminal() {
        let mob_id = MobId::from("kickoff-chain-order");
        let identity = AgentIdentity::from("placed-worker");
        let obligation = test_placed_kickoff_obligation(&identity);
        let starting = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Starting,
            error: None,
            updated_at: UNIX_EPOCH,
        };
        let events = vec![
            test_event(
                &mob_id,
                1,
                MobEventKind::PlacedKickoffObligationRecorded {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                2,
                MobEventKind::MemberKickoffUpdated {
                    member: identity,
                    kickoff: starting,
                },
            ),
            test_event(
                &mob_id,
                3,
                MobEventKind::PlacedKickoffOutcomeAcknowledged { obligation },
            ),
        ];
        let error = collect_placed_kickoff_recovery_chains(&events)
            .expect_err("ACK without a preceding resolved terminal must fail closed");
        assert!(error.to_string().contains("outside Resolved custody"));
    }

    #[test]
    fn placed_kickoff_recovery_rejects_partial_structural_pair() {
        let mob_id = MobId::from("kickoff-partial-pair");
        let identity = AgentIdentity::from("placed-worker");
        let obligation = test_placed_kickoff_obligation(&identity);
        let events = vec![test_event(
            &mob_id,
            1,
            MobEventKind::PlacedKickoffObligationRecorded { obligation },
        )];
        let error = collect_placed_kickoff_recovery_chains(&events)
            .expect_err("a partial Record batch must fail closed");
        assert!(error.to_string().contains("no atomic kickoff projection"));
    }

    #[test]
    fn placed_kickoff_recovery_accepts_empty_authenticated_failure_detail() {
        let mob_id = MobId::from("kickoff-empty-failure");
        let identity = AgentIdentity::from("placed-worker");
        let obligation = test_placed_kickoff_obligation(&identity);
        let starting = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Starting,
            error: None,
            updated_at: UNIX_EPOCH,
        };
        let failed = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Failed,
            error: Some(String::new()),
            updated_at: UNIX_EPOCH,
        };
        let events = vec![
            test_event(
                &mob_id,
                1,
                MobEventKind::PlacedKickoffObligationRecorded {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                2,
                MobEventKind::MemberKickoffUpdated {
                    member: identity.clone(),
                    kickoff: starting,
                },
            ),
            test_event(
                &mob_id,
                3,
                MobEventKind::PlacedKickoffOutcomeResolved {
                    obligation: obligation.clone(),
                    outcome: crate::event::PlacedKickoffHostOutcomeEvent::InteractionFailed {
                        error: String::new(),
                    },
                    kickoff: failed.clone(),
                },
            ),
            test_event(
                &mob_id,
                4,
                MobEventKind::MemberKickoffUpdated {
                    member: identity,
                    kickoff: failed.clone(),
                },
            ),
        ];
        let chains = collect_placed_kickoff_recovery_chains(&events)
            .expect("empty authenticated failure text remains a valid terminal");
        let chain = chains
            .get(&PlacedKickoffRecoveryKey::from_event(&obligation))
            .expect("exact chain is present");
        let resolution = chain.resolved.as_ref().unwrap();
        validate_recovered_kickoff_resolution(
            &obligation,
            &resolution.outcome,
            &resolution.kickoff,
        )
        .expect("empty failure detail must not strand Pending custody");
    }

    #[test]
    fn placed_kickoff_recovery_rejects_host_outcome_lifecycle_drift() {
        let mob_id = MobId::from("kickoff-outcome-lifecycle-drift");
        let identity = AgentIdentity::from("placed-worker");
        let obligation = test_placed_kickoff_obligation(&identity);
        let starting = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Starting,
            error: None,
            updated_at: UNIX_EPOCH,
        };
        let callback = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::CallbackPending,
            error: None,
            updated_at: UNIX_EPOCH,
        };
        let events = vec![
            test_event(
                &mob_id,
                1,
                MobEventKind::PlacedKickoffObligationRecorded {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                2,
                MobEventKind::MemberKickoffUpdated {
                    member: identity.clone(),
                    kickoff: starting,
                },
            ),
            test_event(
                &mob_id,
                3,
                MobEventKind::PlacedKickoffOutcomeResolved {
                    obligation,
                    outcome: crate::event::PlacedKickoffHostOutcomeEvent::InteractionComplete,
                    kickoff: callback.clone(),
                },
            ),
            test_event(
                &mob_id,
                4,
                MobEventKind::MemberKickoffUpdated {
                    member: identity,
                    kickoff: callback,
                },
            ),
        ];
        let error = collect_placed_kickoff_recovery_chains(&events)
            .expect_err("Complete must not recover through a CallbackPending projection");
        assert!(
            error
                .to_string()
                .contains("host outcome does not match its lifecycle projection")
        );
    }

    #[test]
    fn placed_kickoff_recovery_rejects_no_effect_error_drift() {
        let mob_id = MobId::from("kickoff-rejection-error-drift");
        let identity = AgentIdentity::from("placed-worker");
        let obligation = test_placed_kickoff_obligation(&identity);
        let starting = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Starting,
            error: None,
            updated_at: UNIX_EPOCH,
        };
        let failed = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Failed,
            error: Some("projected error".to_string()),
            updated_at: UNIX_EPOCH,
        };
        let events = vec![
            test_event(
                &mob_id,
                1,
                MobEventKind::PlacedKickoffObligationRecorded {
                    obligation: obligation.clone(),
                },
            ),
            test_event(
                &mob_id,
                2,
                MobEventKind::MemberKickoffUpdated {
                    member: identity.clone(),
                    kickoff: starting,
                },
            ),
            test_event(
                &mob_id,
                3,
                MobEventKind::PlacedKickoffRejectedNoEffect {
                    obligation,
                    error: "different authenticated error".to_string(),
                    kickoff: failed.clone(),
                },
            ),
            test_event(
                &mob_id,
                4,
                MobEventKind::MemberKickoffUpdated {
                    member: identity,
                    kickoff: failed,
                },
            ),
        ];
        let error = collect_placed_kickoff_recovery_chains(&events)
            .expect_err("recovery must not mix the rejection detail and lifecycle error");
        assert!(
            error
                .to_string()
                .contains("no-effect rejection does not match its lifecycle projection")
        );
    }

    #[derive(Debug, Clone, Copy)]
    enum KickoffCrashWindow {
        CarrierOnly,
        RecordedStarting,
        ResolvedUnacked,
        ResolvedEmptyFailure,
        ResolvedCancelledComplete,
        ResolvedCancelledCallbackPending,
        ResolvedCancelledFailed,
        ResolvedHostCancelled,
        Acknowledged,
        AcknowledgedDormantBinding,
        Disposed,
        RejectedNoEffect,
        RejectedNoEffectCancelled,
        CancelledPending,
    }

    async fn append_kickoff_pair(
        store: &Arc<dyn MobEventStore>,
        mob_id: &MobId,
        first: MobEventKind,
        identity: &AgentIdentity,
        kickoff: &MobMemberKickoffSnapshot,
    ) {
        store
            .append_batch(vec![
                NewMobEvent {
                    mob_id: mob_id.clone(),
                    timestamp: None,
                    kind: first,
                },
                NewMobEvent {
                    mob_id: mob_id.clone(),
                    timestamp: None,
                    kind: MobEventKind::MemberKickoffUpdated {
                        member: identity.clone(),
                        kickoff: kickoff.clone(),
                    },
                },
            ])
            .await
            .expect("append exact kickoff structural pair");
    }

    async fn recover_kickoff_crash_window(
        case: KickoffCrashWindow,
    ) -> (
        mob_dsl::MobMachineAuthority,
        Vec<MobEvent>,
        crate::event::PlacedKickoffObligationEvent,
    ) {
        let mob_id = MobId::from(format!("kickoff-recovery-{case:?}"));
        let identity = AgentIdentity::from("placed-worker");
        let record = test_autonomous_placed_carrier(&mob_id, &identity);
        let plan = test_current_carrier_plan(&record);
        let obligation = super::placed_kickoff_reconciler::obligation_event_from_carrier(&record)
            .expect("derive exact kickoff obligation");
        let starting = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Starting,
            error: None,
            updated_at: UNIX_EPOCH,
        };
        let started = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Started,
            error: None,
            updated_at: UNIX_EPOCH,
        };
        let cancelled = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Cancelled,
            error: None,
            updated_at: UNIX_EPOCH,
        };
        let failed = MobMemberKickoffSnapshot {
            objective_id: Some(obligation.objective_id),
            phase: MobMemberKickoffPhase::Failed,
            error: Some("certified rejection".to_string()),
            updated_at: UNIX_EPOCH,
        };
        let empty_failed = MobMemberKickoffSnapshot {
            error: Some(String::new()),
            ..failed.clone()
        };
        let store: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        store
            .append(repaired_placed_spawn_event(&mob_id, &record).expect("spawn projection"))
            .await
            .expect("append exact MemberSpawned projection");

        if !matches!(case, KickoffCrashWindow::CarrierOnly) {
            append_kickoff_pair(
                &store,
                &mob_id,
                MobEventKind::PlacedKickoffObligationRecorded {
                    obligation: obligation.clone(),
                },
                &identity,
                &starting,
            )
            .await;
        }
        match case {
            KickoffCrashWindow::ResolvedUnacked
            | KickoffCrashWindow::Acknowledged
            | KickoffCrashWindow::AcknowledgedDormantBinding => {
                append_kickoff_pair(
                    &store,
                    &mob_id,
                    MobEventKind::PlacedKickoffOutcomeResolved {
                        obligation: obligation.clone(),
                        outcome: crate::event::PlacedKickoffHostOutcomeEvent::InteractionComplete,
                        kickoff: started.clone(),
                    },
                    &identity,
                    &started,
                )
                .await;
                if matches!(
                    case,
                    KickoffCrashWindow::Acknowledged
                        | KickoffCrashWindow::AcknowledgedDormantBinding
                ) {
                    store
                        .append(NewMobEvent {
                            mob_id: mob_id.clone(),
                            timestamp: None,
                            kind: MobEventKind::PlacedKickoffOutcomeAcknowledged {
                                obligation: obligation.clone(),
                            },
                        })
                        .await
                        .expect("append kickoff ACK");
                }
            }
            KickoffCrashWindow::ResolvedEmptyFailure => {
                append_kickoff_pair(
                    &store,
                    &mob_id,
                    MobEventKind::PlacedKickoffOutcomeResolved {
                        obligation: obligation.clone(),
                        outcome: crate::event::PlacedKickoffHostOutcomeEvent::InteractionFailed {
                            error: String::new(),
                        },
                        kickoff: empty_failed.clone(),
                    },
                    &identity,
                    &empty_failed,
                )
                .await;
            }
            KickoffCrashWindow::Disposed => {
                append_kickoff_pair(
                    &store,
                    &mob_id,
                    MobEventKind::PlacedKickoffOutcomeDisposed {
                        obligation: obligation.clone(),
                    },
                    &identity,
                    &cancelled,
                )
                .await;
            }
            KickoffCrashWindow::RejectedNoEffect => {
                append_kickoff_pair(
                    &store,
                    &mob_id,
                    MobEventKind::PlacedKickoffRejectedNoEffect {
                        obligation: obligation.clone(),
                        error: "certified rejection".to_string(),
                        kickoff: failed.clone(),
                    },
                    &identity,
                    &failed,
                )
                .await;
            }
            KickoffCrashWindow::ResolvedCancelledComplete
            | KickoffCrashWindow::ResolvedCancelledCallbackPending
            | KickoffCrashWindow::ResolvedCancelledFailed
            | KickoffCrashWindow::ResolvedHostCancelled
            | KickoffCrashWindow::RejectedNoEffectCancelled => {
                store
                    .append(NewMobEvent {
                        mob_id: mob_id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MemberKickoffUpdated {
                            member: identity.clone(),
                            kickoff: cancelled.clone(),
                        },
                    })
                    .await
                    .expect("append durable Stop cancellation before host terminal");
                let terminal = match case {
                    KickoffCrashWindow::ResolvedCancelledComplete => {
                        MobEventKind::PlacedKickoffOutcomeResolved {
                            obligation: obligation.clone(),
                            outcome:
                                crate::event::PlacedKickoffHostOutcomeEvent::InteractionComplete,
                            kickoff: cancelled.clone(),
                        }
                    }
                    KickoffCrashWindow::ResolvedCancelledCallbackPending => {
                        MobEventKind::PlacedKickoffOutcomeResolved {
                            obligation: obligation.clone(),
                            outcome: crate::event::PlacedKickoffHostOutcomeEvent::InteractionCallbackPending,
                            kickoff: cancelled.clone(),
                        }
                    }
                    KickoffCrashWindow::ResolvedCancelledFailed => {
                        MobEventKind::PlacedKickoffOutcomeResolved {
                            obligation: obligation.clone(),
                            outcome: crate::event::PlacedKickoffHostOutcomeEvent::InteractionFailed {
                                error: "host failure after cancellation".to_string(),
                            },
                            kickoff: cancelled.clone(),
                        }
                    }
                    KickoffCrashWindow::ResolvedHostCancelled => {
                        MobEventKind::PlacedKickoffOutcomeResolved {
                            obligation: obligation.clone(),
                            outcome:
                                crate::event::PlacedKickoffHostOutcomeEvent::InteractionCancelled,
                            kickoff: cancelled.clone(),
                        }
                    }
                    KickoffCrashWindow::RejectedNoEffectCancelled => {
                        MobEventKind::PlacedKickoffRejectedNoEffect {
                            obligation: obligation.clone(),
                            error: "certified rejection after cancellation".to_string(),
                            kickoff: cancelled.clone(),
                        }
                    }
                    _ => unreachable!(),
                };
                append_kickoff_pair(&store, &mob_id, terminal, &identity, &cancelled).await;
            }
            KickoffCrashWindow::CancelledPending => {
                store
                    .append(NewMobEvent {
                        mob_id: mob_id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MemberKickoffUpdated {
                            member: identity,
                            kickoff: cancelled,
                        },
                    })
                    .await
                    .expect("append durable Stop cancellation");
            }
            KickoffCrashWindow::CarrierOnly | KickoffCrashWindow::RecordedStarting => {}
        }

        let mut events = store.replay_all().await.expect("replay crash window");
        let mut authority = if matches!(case, KickoffCrashWindow::AcknowledgedDormantBinding) {
            test_authority_for_current_carrier_at_host_generation(
                &record,
                &plan,
                record.host_binding_generation + 1,
            )
        } else {
            test_authority_for_current_carrier(&record, &plan)
        };
        recover_placed_kickoff_outcome_custody(&mut authority, &store, &mob_id, &mut events, &plan)
            .await
            .expect("recover exact placed kickoff crash window");
        (authority, events, obligation)
    }

    #[tokio::test]
    async fn placed_kickoff_cold_recovery_covers_every_custody_crash_window() {
        for case in [
            KickoffCrashWindow::CarrierOnly,
            KickoffCrashWindow::RecordedStarting,
            KickoffCrashWindow::ResolvedUnacked,
            KickoffCrashWindow::ResolvedEmptyFailure,
            KickoffCrashWindow::ResolvedCancelledComplete,
            KickoffCrashWindow::ResolvedCancelledCallbackPending,
            KickoffCrashWindow::ResolvedCancelledFailed,
            KickoffCrashWindow::ResolvedHostCancelled,
            KickoffCrashWindow::Acknowledged,
            KickoffCrashWindow::AcknowledgedDormantBinding,
            KickoffCrashWindow::Disposed,
            KickoffCrashWindow::RejectedNoEffect,
            KickoffCrashWindow::RejectedNoEffectCancelled,
            KickoffCrashWindow::CancelledPending,
        ] {
            let (mut authority, events, obligation_event) =
                recover_kickoff_crash_window(case).await;
            let obligation =
                super::remote_flow_ticket::placed_kickoff_obligation_from_event(&obligation_event);
            let state = authority.state();
            let pending = state.pending_placed_kickoff_outcomes.contains(&obligation);
            let resolved = state.resolved_placed_kickoff_outcomes.contains(&obligation);
            match case {
                KickoffCrashWindow::CarrierOnly | KickoffCrashWindow::RecordedStarting => {
                    assert!(
                        pending && !resolved,
                        "{case:?} must recover Pending custody"
                    );
                    assert!(
                        state
                            .member_kickoff_starting
                            .contains(&obligation.agent_identity),
                        "{case:?} must recover Starting lifecycle"
                    );
                }
                KickoffCrashWindow::ResolvedUnacked => {
                    assert!(
                        !pending && resolved,
                        "resolved terminal remains ACK-pending"
                    );
                    assert!(
                        state
                            .member_kickoff_started
                            .contains(&obligation.agent_identity)
                    );
                }
                KickoffCrashWindow::ResolvedEmptyFailure => {
                    assert!(
                        !pending && resolved,
                        "empty authenticated failure remains ACK-pending"
                    );
                    assert!(
                        state
                            .member_kickoff_failed
                            .contains(&obligation.agent_identity),
                        "empty failure detail must still terminalize kickoff"
                    );
                    assert_eq!(
                        state
                            .member_kickoff_error
                            .get(&obligation.agent_identity)
                            .map(String::as_str),
                        Some("")
                    );
                }
                KickoffCrashWindow::ResolvedCancelledComplete
                | KickoffCrashWindow::ResolvedCancelledCallbackPending
                | KickoffCrashWindow::ResolvedCancelledFailed
                | KickoffCrashWindow::ResolvedHostCancelled => {
                    assert!(
                        !pending && resolved,
                        "{case:?} must retain exact host ACK custody"
                    );
                    assert!(
                        state
                            .member_kickoff_cancelled
                            .contains(&obligation.agent_identity),
                        "{case:?} must preserve cancellation as the public lifecycle winner"
                    );
                    let expected = match case {
                        KickoffCrashWindow::ResolvedCancelledComplete => {
                            mob_dsl::PlacedKickoffOutcomeKind::Started
                        }
                        KickoffCrashWindow::ResolvedCancelledCallbackPending => {
                            mob_dsl::PlacedKickoffOutcomeKind::CallbackPending
                        }
                        KickoffCrashWindow::ResolvedCancelledFailed => {
                            mob_dsl::PlacedKickoffOutcomeKind::Failed
                        }
                        KickoffCrashWindow::ResolvedHostCancelled => {
                            mob_dsl::PlacedKickoffOutcomeKind::Cancelled
                        }
                        _ => unreachable!(),
                    };
                    assert_eq!(
                        state
                            .member_placed_kickoff_outcome_kinds
                            .get(&obligation.agent_identity),
                        Some(&expected),
                        "{case:?} must recover the exact host outcome independently of lifecycle"
                    );
                    if matches!(case, KickoffCrashWindow::ResolvedCancelledFailed) {
                        assert_eq!(
                            state
                                .member_placed_kickoff_outcome_errors
                                .get(&obligation.agent_identity)
                                .map(String::as_str),
                            Some("host failure after cancellation")
                        );
                    }
                }
                KickoffCrashWindow::Acknowledged
                | KickoffCrashWindow::AcknowledgedDormantBinding => {
                    assert!(!pending && !resolved, "ACK closes exact custody");
                    assert!(
                        state
                            .member_kickoff_started
                            .contains(&obligation.agent_identity)
                    );
                }
                KickoffCrashWindow::Disposed => {
                    assert!(!pending && !resolved, "Dispose closes exact custody");
                    assert!(
                        state
                            .member_kickoff_cancelled
                            .contains(&obligation.agent_identity)
                    );
                }
                KickoffCrashWindow::RejectedNoEffect => {
                    assert!(!pending && !resolved, "certified rejection closes custody");
                    assert!(
                        state
                            .member_kickoff_failed
                            .contains(&obligation.agent_identity)
                    );
                }
                KickoffCrashWindow::RejectedNoEffectCancelled => {
                    assert!(!pending && !resolved, "certified rejection closes custody");
                    assert!(
                        state
                            .member_kickoff_cancelled
                            .contains(&obligation.agent_identity),
                        "cancellation remains the public lifecycle winner"
                    );
                    assert_eq!(
                        state
                            .member_placed_kickoff_outcome_kinds
                            .get(&obligation.agent_identity),
                        Some(&mob_dsl::PlacedKickoffOutcomeKind::RejectedNoEffect)
                    );
                    assert_eq!(
                        state
                            .member_placed_kickoff_outcome_errors
                            .get(&obligation.agent_identity)
                            .map(String::as_str),
                        Some("certified rejection after cancellation"),
                        "recovery must not invent a placeholder rejection detail"
                    );
                }
                KickoffCrashWindow::CancelledPending => {
                    assert!(pending && !resolved, "Stop retains possible host custody");
                    assert!(
                        state
                            .member_kickoff_cancelled
                            .contains(&obligation.agent_identity),
                        "Stop cancellation must survive cold recovery"
                    );
                }
            }
            assert_eq!(
                state
                    .member_kickoff_input_ids
                    .get(&obligation.agent_identity),
                Some(&obligation.input_id),
                "{case:?} must retain the exact durable input id"
            );
            if matches!(case, KickoffCrashWindow::CarrierOnly) {
                assert!(
                    events.iter().any(|event| matches!(
                        &event.kind,
                        MobEventKind::PlacedKickoffObligationRecorded { obligation }
                            if obligation == &obligation_event
                    )),
                    "carrier-only recovery must durably repair Record before actor startup"
                );
            }
            let replay = match case {
                KickoffCrashWindow::ResolvedCancelledComplete => {
                    Some(mob_dsl::MobMachineInput::ResolvePlacedKickoffStarted {
                        obligation: obligation.clone(),
                    })
                }
                KickoffCrashWindow::ResolvedCancelledCallbackPending => Some(
                    mob_dsl::MobMachineInput::ResolvePlacedKickoffCallbackPending {
                        obligation: obligation.clone(),
                    },
                ),
                KickoffCrashWindow::ResolvedCancelledFailed => {
                    Some(mob_dsl::MobMachineInput::ResolvePlacedKickoffFailed {
                        obligation: obligation.clone(),
                        error: "host failure after cancellation".to_string(),
                    })
                }
                KickoffCrashWindow::ResolvedHostCancelled => {
                    Some(mob_dsl::MobMachineInput::ResolvePlacedKickoffCancelled {
                        obligation: obligation.clone(),
                    })
                }
                KickoffCrashWindow::RejectedNoEffectCancelled => Some(
                    mob_dsl::MobMachineInput::RejectPlacedKickoffBeforeAdmission {
                        obligation: obligation.clone(),
                        error: "certified rejection after cancellation".to_string(),
                    },
                ),
                _ => None,
            };
            if let Some(replay) = replay {
                apply_seeded_mob_input(
                    &mut authority,
                    replay,
                    "verify_cancelled_kickoff_exact_terminal_replay",
                )
                .expect("cold recovery must accept the exact retained host terminal replay");
            }
        }
    }

    fn install_test_placed_member(
        authority: &mut mob_dsl::MobMachineAuthority,
        identity: &AgentIdentity,
        generation: Generation,
        fence_token: FenceToken,
        host: &mob_dsl::HostId,
        member_session_id: &str,
    ) {
        let placed_spawn_id = crate::ids::PlacedSpawnId::new();
        if authority.state().owner_bridge_session_id.is_none() {
            authority
                .apply_signal(mob_dsl::MobMachineSignal::RecoverOwnerBridgeSession {
                    bridge_session_id: mob_dsl::SessionId("test-owner-session".to_string()),
                    destroy_on_owner_archive: false,
                    implicit_delegation_mob: false,
                })
                .expect("recover owner for placed replay");
        }
        authority
            .apply_signal(mob_dsl::MobMachineSignal::RecoverHostBinding {
                host_id: host.clone(),
                pubkey: mob_dsl::PeerSigningKey([9; 32]),
                endpoint: mob_dsl::PeerAddress(format!("tcp://hosts/{}", host.0)),
                epoch: 7,
                binding_generation: 1,
                protocol_min: 4,
                protocol_max: 4,
                engine_version: "placed-recovery-test-engine".to_string(),
                durable_sessions: true,
                autonomous_members: true,
                hard_cancel_member: true,
                tracked_input_cancel: true,
                memory_store: true,
                mcp: true,
                resolvable_providers: BTreeSet::from(["anthropic".to_string()]),
                approval_forwarding: false,
                live_endpoint: None,
            })
            .expect("recover bound host for placed replay");
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&AgentRuntimeId::new(
            identity.clone(),
            generation,
        ));
        let mut signing_key = [3; 32];
        signing_key[0] = (fence_token.get() % 251) as u8;
        let peer_id = meerkat_core::comms::PeerId::from_ed25519_pubkey(&signing_key).to_string();
        let operation_owner_session_id = authority
            .state()
            .owner_bridge_session_id
            .clone()
            .expect("placed replay owner is recovered");
        authority
            .apply_signal(mob_dsl::MobMachineSignal::RecoverCommittedPlacedSpawn {
                spawn_id: mob_dsl::PlacedSpawnId(placed_spawn_id.to_string()),
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: dsl_runtime_id,
                generation: mob_dsl::Generation::from_domain(generation),
                fence_token: mob_dsl::FenceToken::from_domain(fence_token),
                host_id: host.clone(),
                host_binding_generation: 1,
                member_session_id: mob_dsl::SessionId(member_session_id.to_string()),
                member_peer_endpoint: mob_dsl::MemberPeerEndpoint {
                    name: mob_dsl::PeerName(format!("placed-{}", identity.as_str())),
                    peer_id: mob_dsl::PeerId(peer_id),
                    address: mob_dsl::PeerAddress(format!("tcp://members/{}", identity.as_str())),
                    signing_key: mob_dsl::PeerSigningKey(signing_key),
                },
                profile_name: "worker".to_string(),
                runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::TurnDriven,
                external_addressable: true,
                provision_operation_id: format!("recover-operation-{placed_spawn_id}"),
                operation_owner_session_id,
            })
            .expect("recover exact committed placed member");
    }

    #[test]
    fn committed_generation_zero_placed_spawn_is_deferred_to_placed_recovery() {
        let definition = MobDefinition::explicit("placed-generation-zero");
        let identity = AgentIdentity::from("placed-worker");
        let generation = Generation::INITIAL;
        let fence_token = FenceToken::new(7);
        let placed_spawn_id = crate::ids::PlacedSpawnId::new();
        let events = vec![placed_spawned_event(
            &definition.id,
            1,
            &identity,
            generation,
            fence_token,
            &placed_spawn_id,
        )];
        let committed_keys = BTreeSet::from([(
            identity.as_str().to_string(),
            generation.get(),
            placed_spawn_id,
        )]);
        let mut authority = seed_mob_authority();

        seed_mob_authority_sync_from_events(
            &mut authority,
            &events,
            &definition,
            false,
            &committed_keys,
        )
        .expect("generic replay defers committed placed membership");

        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);
        assert!(
            !authority
                .state()
                .identity_to_runtime
                .contains_key(&dsl_identity),
            "the generic event carrier must not become a second placed membership owner"
        );
        assert!(
            !authority
                .state()
                .identity_runtime_generations
                .contains_key(&dsl_identity),
            "generation zero remains available to the committed placed carrier"
        );
    }

    #[test]
    fn generic_replay_restores_local_endpoint_but_not_placed_endpoint_authority() {
        let definition = MobDefinition::explicit("endpoint-replay-ownership");
        let local_identity = AgentIdentity::from("local-worker");
        let placed_identity = AgentIdentity::from("placed-worker");
        let local_key = [6; 32];
        let local_peer_id = PeerId::from_ed25519_pubkey(&local_key);
        let local_endpoint = TrustedPeerDescriptor::unsigned_with_pubkey(
            "endpoint-replay-ownership/worker/local-worker",
            local_peer_id.to_string(),
            local_key,
            "inproc://endpoint-replay-ownership/worker/local-worker",
        )
        .expect("local endpoint");
        let placed_key = [7; 32];
        let placed_peer_id = PeerId::from_ed25519_pubkey(&placed_key);
        let placed_endpoint = TrustedPeerDescriptor::unsigned_with_pubkey(
            "endpoint-replay-ownership/worker/placed-worker",
            placed_peer_id.to_string(),
            placed_key,
            "inproc://endpoint-replay-ownership/worker/placed-worker",
        )
        .expect("placed endpoint");
        let mut placed_event = placed_spawned_event(
            &definition.id,
            2,
            &placed_identity,
            Generation::INITIAL,
            FenceToken::new(2),
            &crate::ids::PlacedSpawnId::new(),
        );
        let MobEventKind::MemberSpawned(placed_spawned) = &mut placed_event.kind else {
            panic!("placed helper must produce MemberSpawned");
        };
        placed_spawned.member_peer_endpoint = Some(placed_endpoint);
        let events = vec![
            MobEvent {
                cursor: 1,
                timestamp: Utc::now(),
                mob_id: definition.id.clone(),
                kind: MobEventKind::MemberSpawned(
                    MemberSpawnedEvent::new(
                        local_identity.clone(),
                        Generation::INITIAL,
                        FenceToken::new(1),
                        AgentRuntimeId::initial(local_identity.clone()),
                        ProfileName::from("worker"),
                    )
                    .with_member_peer_endpoint(Some(local_endpoint.clone())),
                ),
            },
            placed_event,
        ];
        let mut authority = seed_mob_authority();

        seed_mob_authority_sync_from_events(
            &mut authority,
            &events,
            &definition,
            false,
            &BTreeSet::new(),
        )
        .expect("generic replay preserves endpoint ownership boundaries");

        assert_eq!(
            authority
                .state()
                .member_peer_endpoints
                .get(&mob_dsl::AgentIdentity::from_domain(&local_identity),),
            Some(&mob_dsl::MemberPeerEndpoint::from(&local_endpoint)),
            "local and peer-only spawn endpoints are replayed from lifecycle events",
        );
        assert!(
            !authority
                .state()
                .member_peer_endpoints
                .contains_key(&mob_dsl::AgentIdentity::from_domain(&placed_identity),),
            "placed spawn endpoints must be recovered only from the exact carrier",
        );
    }

    #[test]
    fn committed_later_placed_spawn_defers_only_current_incarnation() {
        let definition = MobDefinition::explicit("placed-later-generation");
        let identity = AgentIdentity::from("placed-worker");
        let generation_zero = Generation::INITIAL;
        let generation_one = Generation::new(1);
        let retired_spawn_id = crate::ids::PlacedSpawnId::new();
        let current_spawn_id = crate::ids::PlacedSpawnId::new();
        let events = vec![
            MobEvent {
                cursor: 1,
                timestamp: Utc::now(),
                mob_id: definition.id.clone(),
                kind: MobEventKind::MobOwnerBridgeSessionBound {
                    bridge_session_id: meerkat_core::SessionId::new(),
                    destroy_on_owner_archive: false,
                    implicit_delegation_mob: false,
                },
            },
            placed_spawned_event(
                &definition.id,
                2,
                &identity,
                generation_zero,
                FenceToken::new(7),
                &retired_spawn_id,
            ),
            MobEvent {
                cursor: 3,
                timestamp: Utc::now(),
                mob_id: definition.id.clone(),
                kind: MobEventKind::MemberRetired {
                    agent_identity: identity.clone(),
                    generation: generation_zero,
                    role: ProfileName::from("worker"),
                },
            },
            placed_spawned_event(
                &definition.id,
                4,
                &identity,
                generation_one,
                FenceToken::new(8),
                &current_spawn_id,
            ),
        ];
        let committed_keys = BTreeSet::from([(
            identity.as_str().to_string(),
            generation_one.get(),
            current_spawn_id,
        )]);
        let mut authority = seed_mob_authority();

        seed_mob_authority_sync_from_events(
            &mut authority,
            &events,
            &definition,
            false,
            &committed_keys,
        )
        .expect("historical generation replays while current placed spawn defers");

        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);
        assert!(
            !authority
                .state()
                .identity_to_runtime
                .contains_key(&dsl_identity)
        );
        assert_eq!(
            authority
                .state()
                .identity_runtime_generations
                .get(&dsl_identity),
            Some(&mob_dsl::Generation::from_domain(generation_zero)),
            "the retired generation remains the exact high-water for placed recovery"
        );
        let host = mob_dsl::HostId("placed-host".to_string());
        install_test_placed_member(
            &mut authority,
            &identity,
            generation_one,
            FenceToken::new(8),
            &host,
            "placed-session-generation-one",
        );
        assert_eq!(
            authority
                .state()
                .identity_runtime_generations
                .get(&dsl_identity),
            Some(&mob_dsl::Generation::from_domain(generation_one)),
            "the committed placed carrier installs the strict successor"
        );
        assert_eq!(
            authority.state().member_placement.get(&dsl_identity),
            Some(&host)
        );
    }

    #[test]
    fn current_placed_retirement_replays_only_after_placed_membership() {
        let definition = MobDefinition::explicit("placed-retiring-recovery");
        let identity = AgentIdentity::from("placed-retiring-worker");
        let generation = Generation::INITIAL;
        let fence_token = FenceToken::new(7);
        let host = mob_dsl::HostId("placed-retiring-host".to_string());
        let member_session_id = meerkat_core::SessionId::new();
        let member_session_id_text = member_session_id.to_string();
        let placed_spawn_id = crate::ids::PlacedSpawnId::new();
        let events = vec![
            MobEvent {
                cursor: 1,
                timestamp: Utc::now(),
                mob_id: definition.id.clone(),
                kind: MobEventKind::MobOwnerBridgeSessionBound {
                    bridge_session_id: meerkat_core::SessionId::new(),
                    destroy_on_owner_archive: false,
                    implicit_delegation_mob: false,
                },
            },
            placed_spawned_event(
                &definition.id,
                2,
                &identity,
                generation,
                fence_token,
                &placed_spawn_id,
            ),
            MobEvent {
                cursor: 3,
                timestamp: Utc::now(),
                mob_id: definition.id.clone(),
                kind: MobEventKind::MemberRetirementStarted {
                    agent_identity: identity.clone(),
                    agent_runtime_id: AgentRuntimeId::new(identity.clone(), generation),
                    generation,
                    role: ProfileName::from("worker"),
                    releasing: None,
                    session_id: Some(member_session_id),
                    retiring_peer_endpoint: None,
                    preserve_machine_topology: false,
                },
            },
        ];
        let committed_keys = BTreeSet::from([(
            identity.as_str().to_string(),
            generation.get(),
            placed_spawn_id,
        )]);
        let mut authority = seed_mob_authority();

        seed_mob_authority_sync_from_events(
            &mut authority,
            &events,
            &definition,
            false,
            &committed_keys,
        )
        .expect("first pass defers both placed membership ownership and retirement start");
        install_test_placed_member(
            &mut authority,
            &identity,
            generation,
            fence_token,
            &host,
            &member_session_id_text,
        );
        recover_pending_member_retirements(&mut authority, &events)
            .expect("retirement start applies against exact recovered placed carriers");

        let dsl_runtime_id =
            mob_dsl::AgentRuntimeId::from_domain(&AgentRuntimeId::new(identity, generation));
        assert_eq!(
            authority.state().member_state_markers.get(&dsl_runtime_id),
            Some(&mob_dsl::MobMemberState::Retiring)
        );
    }

    #[test]
    fn pending_placed_carrier_rejects_any_projectable_spawn_event() {
        let definition = MobDefinition::explicit("pending-placed-recovery");
        let identity = AgentIdentity::from("pending-worker");
        let generation = Generation::INITIAL;
        let fence = FenceToken::new(71);
        let spawn_id = crate::ids::PlacedSpawnId::new();
        let record = test_placed_carrier(
            &definition.id,
            &identity,
            generation,
            fence,
            spawn_id.clone(),
            false,
        );
        let events = vec![placed_spawned_event(
            &definition.id,
            1,
            &identity,
            generation,
            fence,
            &spawn_id,
        )];

        let error = classify_placed_spawn_recovery(&definition.id, &events, &[record])
            .expect_err("Pending carrier plus spawn projection must fail closed");
        assert!(error.to_string().contains("Pending placed carrier"));
    }

    #[test]
    fn committed_carrier_rejects_mismatched_or_duplicate_spawn_ids() {
        let definition = MobDefinition::explicit("placed-spawn-id-conflicts");
        let identity = AgentIdentity::from("placed-worker");
        let generation = Generation::INITIAL;
        let fence = FenceToken::new(72);
        let carrier_spawn_id = crate::ids::PlacedSpawnId::new();
        let other_spawn_id = crate::ids::PlacedSpawnId::new();
        let record = test_placed_carrier(
            &definition.id,
            &identity,
            generation,
            fence,
            carrier_spawn_id,
            true,
        );
        let mismatched = vec![placed_spawned_event(
            &definition.id,
            1,
            &identity,
            generation,
            fence,
            &other_spawn_id,
        )];
        let error = classify_placed_spawn_recovery(
            &definition.id,
            &mismatched,
            std::slice::from_ref(&record),
        )
        .expect_err("current mismatched spawn id must fail closed");
        assert!(error.to_string().contains("missing its exact event"));

        let first = exact_placed_spawned_event(&definition.id, 1, &record);
        let mut duplicate = first.clone();
        duplicate.cursor = 2;
        let error = classify_placed_spawn_recovery(
            &definition.id,
            &[first, duplicate],
            std::slice::from_ref(&record),
        )
        .expect_err("duplicate exact spawn projection must fail closed");
        assert!(error.to_string().contains("duplicate placed MemberSpawned"));
    }

    #[test]
    fn committed_carrier_requires_exact_spawn_link_before_retirement_cleanup() {
        let definition = MobDefinition::explicit("placed-retire-link");
        let identity = AgentIdentity::from("placed-worker");
        let generation = Generation::INITIAL;
        let record = test_placed_carrier(
            &definition.id,
            &identity,
            generation,
            FenceToken::new(73),
            crate::ids::PlacedSpawnId::new(),
            true,
        );
        let retirement = retired_event(&definition.id, 1, &identity, generation);

        let error = classify_placed_spawn_recovery(
            &definition.id,
            &[retirement],
            std::slice::from_ref(&record),
        )
        .expect_err("same-generation retirement without exact spawn link is ambiguous");
        assert!(
            error
                .to_string()
                .contains("no exact placed MemberSpawned link")
        );
    }

    #[test]
    fn placed_spawn_after_same_generation_retirement_is_rejected_as_resurrection() {
        let definition = MobDefinition::explicit("placed-resurrection");
        let identity = AgentIdentity::from("placed-worker");
        let generation = Generation::INITIAL;
        let record = test_placed_carrier(
            &definition.id,
            &identity,
            generation,
            FenceToken::new(74),
            crate::ids::PlacedSpawnId::new(),
            true,
        );
        let events = vec![
            retired_event(&definition.id, 1, &identity, generation),
            exact_placed_spawned_event(&definition.id, 2, &record),
        ];

        let error =
            classify_placed_spawn_recovery(&definition.id, &events, std::slice::from_ref(&record))
                .expect_err("retired generation cannot be resurrected");
        assert!(error.to_string().contains("resurrects retired identity"));
    }

    #[test]
    fn historical_retired_placed_event_without_carrier_is_allowed() {
        let definition = MobDefinition::explicit("historical-placed-no-carrier");
        let identity = AgentIdentity::from("retired-worker");
        let generation = Generation::INITIAL;
        let spawn_id = crate::ids::PlacedSpawnId::new();
        let events = vec![
            placed_spawned_event(
                &definition.id,
                1,
                &identity,
                generation,
                FenceToken::new(75),
                &spawn_id,
            ),
            retired_event(&definition.id, 2, &identity, generation),
        ];

        let (plan, repairs) = classify_placed_spawn_recovery(&definition.id, &events, &[])
            .expect("historical terminal event may outlive its deleted carrier");
        assert!(plan.entries.is_empty());
        assert!(repairs.is_empty());
    }

    #[test]
    fn committed_missing_event_repairs_deterministically_and_reseeds_fence() {
        let definition = MobDefinition::explicit("placed-event-repair");
        let identity = AgentIdentity::from("placed-worker");
        let mut record = test_placed_carrier(
            &definition.id,
            &identity,
            Generation::INITIAL,
            FenceToken::new(900),
            crate::ids::PlacedSpawnId::new(),
            true,
        );
        record.effective_model_override_present = true;
        record
            .validate_for_mob(&definition.id)
            .expect("model-override carrier remains valid");

        let mut mismatched = exact_placed_spawned_event(&definition.id, 1, &record);
        let MobEventKind::MemberSpawned(mismatched_spawned) = &mut mismatched.kind else {
            panic!("exact placed event helper must produce MemberSpawned");
        };
        mismatched_spawned.effective_model_override = None;
        let mismatch_error = classify_placed_spawn_recovery(
            &definition.id,
            &[mismatched],
            std::slice::from_ref(&record),
        )
        .expect_err("committed event must match the carrier's model-override presence");
        assert!(
            mismatch_error
                .to_string()
                .contains("does not exactly match committed carrier")
        );

        let (plan, repairs) =
            classify_placed_spawn_recovery(&definition.id, &[], std::slice::from_ref(&record))
                .expect("committed carrier deterministically repairs a missing event");
        assert_eq!(repairs.len(), 1);
        assert_eq!(plan.next_carrier_fence_token, 901);
        assert_eq!(
            plan.entries[0].disposition,
            PlacedCarrierRecoveryDisposition::CommittedCurrent
        );
        let MobEventKind::MemberSpawned(repaired_spawned) = &repairs[0].kind else {
            panic!("placed carrier repair must emit MemberSpawned");
        };
        assert_eq!(
            repaired_spawned.effective_model_override.as_deref(),
            Some(record.spec.profile.model.as_str()),
            "cold repair rehydrates the exact digest-covered model override"
        );

        let repaired = MobEvent {
            cursor: 1,
            timestamp: Utc::now(),
            mob_id: definition.id.clone(),
            kind: repairs[0].kind.clone(),
        };
        let (reloaded, second_repairs) = classify_placed_spawn_recovery(
            &definition.id,
            &[repaired],
            std::slice::from_ref(&record),
        )
        .expect("reloaded repaired event is exact");
        assert!(second_repairs.is_empty());
        assert!(reloaded.current_committed_event_keys.contains(&(
            identity.as_str().to_string(),
            Generation::INITIAL.get(),
            record.spawn_id.clone(),
        )));
    }

    #[test]
    fn combined_profile_and_model_override_cold_projection_is_exact() {
        let definition = MobDefinition::explicit("placed-combined-overrides");
        let identity = AgentIdentity::from("placed-worker");
        let mut record = test_placed_carrier(
            &definition.id,
            &identity,
            Generation::INITIAL,
            FenceToken::new(901),
            crate::ids::PlacedSpawnId::new(),
            true,
        );
        record.spec.profile.model = "final-composed-model".to_string();
        record.spec.profile.peer_description = "full override marker".to_string();
        record.spec_digest = meerkat_contracts::wire::portable_member_spec_digest(&record.spec)
            .expect("recompute combined-override carrier digest");
        record.effective_profile_override_present = true;
        record.effective_model_override_present = true;
        record
            .validate_for_mob(&definition.id)
            .expect("combined-override carrier is valid");

        let exact = exact_placed_spawned_event(&definition.id, 1, &record);
        let MobEventKind::MemberSpawned(exact_spawned) = &exact.kind else {
            panic!("exact placed event helper must produce MemberSpawned");
        };
        assert_eq!(
            exact_spawned
                .effective_profile_override
                .as_ref()
                .map(|profile| profile.model.as_str()),
            Some("final-composed-model")
        );
        assert_eq!(
            exact_spawned.effective_model_override.as_deref(),
            Some("final-composed-model")
        );
        classify_placed_spawn_recovery(
            &definition.id,
            std::slice::from_ref(&exact),
            std::slice::from_ref(&record),
        )
        .expect("final composed event matches cold carrier exactly");

        let mut stale_pre_model = exact;
        let MobEventKind::MemberSpawned(stale_spawned) = &mut stale_pre_model.kind else {
            panic!("exact placed event helper must produce MemberSpawned");
        };
        stale_spawned
            .effective_profile_override
            .as_mut()
            .expect("full override is present")
            .model = "pre-model-full-override".to_string();
        let error = classify_placed_spawn_recovery(
            &definition.id,
            &[stale_pre_model],
            std::slice::from_ref(&record),
        )
        .expect_err("pre-model full override must not survive as cold truth");
        assert!(
            error
                .to_string()
                .contains("does not exactly match committed carrier")
        );
    }

    #[test]
    fn recovered_machine_fence_seed_preserves_exhausted_sentinel() {
        let mut state = seed_mob_authority().state().clone();
        let identity = mob_dsl::AgentIdentity::from("fence-max");
        state
            .identity_runtime_fence_tokens
            .insert(identity.clone(), mob_dsl::FenceToken(u64::MAX - 1));
        assert_eq!(next_fence_token_seed(&state), u64::MAX);
        state
            .identity_runtime_fence_tokens
            .insert(identity, mob_dsl::FenceToken(u64::MAX));
        assert_eq!(
            next_fence_token_seed(&state),
            0,
            "cold recovery uses zero only as the exhausted machine sentinel"
        );
        assert_eq!(combine_recovered_fence_token_seeds(0, 41), 0);
        assert_eq!(combine_recovered_fence_token_seeds(41, 0), 0);
        assert_eq!(combine_recovered_fence_token_seeds(41, 42), 42);
    }

    #[test]
    fn recovered_pending_carrier_at_max_preserves_cleanup_and_exhausted_sentinel() {
        let definition = MobDefinition::explicit("placed-carrier-fence-exhaustion");
        let identity = AgentIdentity::from("placed-worker");
        let max_minus_one = test_placed_carrier(
            &definition.id,
            &identity,
            Generation::INITIAL,
            FenceToken::new(u64::MAX - 1),
            crate::ids::PlacedSpawnId::new(),
            false,
        );
        let (plan, repairs) = classify_placed_spawn_recovery(
            &definition.id,
            &[],
            std::slice::from_ref(&max_minus_one),
        )
        .expect("MAX remains a valid next carrier fence");
        assert!(repairs.is_empty());
        assert_eq!(plan.next_carrier_fence_token, u64::MAX);

        let exhausted = test_placed_carrier(
            &definition.id,
            &identity,
            Generation::INITIAL,
            FenceToken::new(u64::MAX),
            crate::ids::PlacedSpawnId::new(),
            false,
        );
        let (exhausted_plan, exhausted_repairs) =
            classify_placed_spawn_recovery(&definition.id, &[], std::slice::from_ref(&exhausted))
                .expect("an exhausted Pending carrier still requires cold cleanup");
        assert!(exhausted_repairs.is_empty());
        assert_eq!(exhausted_plan.next_carrier_fence_token, 0);
        assert_eq!(
            exhausted_plan.entries[0].disposition,
            PlacedCarrierRecoveryDisposition::PendingCleanup,
            "fence exhaustion must not strand an uncertain remote attempt"
        );
    }

    #[test]
    fn repaired_reload_preserves_exhausted_fence_after_pending_max_cleanup() {
        let definition = MobDefinition::explicit("placed-mixed-fence-exhaustion-repair");
        let pending_identity = AgentIdentity::from("pending-max");
        let committed_identity = AgentIdentity::from("committed-repair");
        let pending_max = test_placed_carrier(
            &definition.id,
            &pending_identity,
            Generation::INITIAL,
            FenceToken::new(u64::MAX),
            crate::ids::PlacedSpawnId::new(),
            false,
        );
        let committed = test_placed_carrier(
            &definition.id,
            &committed_identity,
            Generation::INITIAL,
            FenceToken::new(41),
            crate::ids::PlacedSpawnId::new(),
            true,
        );

        let (initial_plan, repairs) = classify_placed_spawn_recovery(
            &definition.id,
            &[],
            &[pending_max.clone(), committed.clone()],
        )
        .expect("Pending MAX cleanup and committed event repair coexist");
        assert_eq!(initial_plan.next_carrier_fence_token, 0);
        assert_eq!(repairs.len(), 1);
        assert!(initial_plan.entries.iter().any(|entry| {
            entry.record.spawn_id == pending_max.spawn_id
                && entry.disposition == PlacedCarrierRecoveryDisposition::PendingCleanup
        }));
        assert!(initial_plan.entries.iter().any(|entry| {
            entry.record.spawn_id == committed.spawn_id
                && entry.disposition == PlacedCarrierRecoveryDisposition::CommittedCurrent
        }));

        let repaired = MobEvent {
            cursor: 1,
            timestamp: Utc::now(),
            mob_id: definition.id.clone(),
            kind: repairs[0].kind.clone(),
        };
        let (mut reloaded_plan, second_repairs) = classify_placed_spawn_recovery(
            &definition.id,
            &[repaired],
            std::slice::from_ref(&committed),
        )
        .expect("reload after Pending cleanup sees the repaired committed carrier");
        assert!(second_repairs.is_empty());
        assert_eq!(reloaded_plan.next_carrier_fence_token, 42);
        reloaded_plan.next_carrier_fence_token = combine_recovered_fence_token_seeds(
            reloaded_plan.next_carrier_fence_token,
            initial_plan.next_carrier_fence_token,
        );
        assert_eq!(
            reloaded_plan.next_carrier_fence_token, 0,
            "repair reload must not reopen allocation after MAX was consumed"
        );
    }

    #[tokio::test]
    async fn missing_owner_or_bound_host_rejects_repair_before_event_append() {
        let definition = MobDefinition::explicit("placed-repair-prerequisites");
        let identity = AgentIdentity::from("placed-worker");
        let record = test_placed_carrier(
            &definition.id,
            &identity,
            Generation::INITIAL,
            FenceToken::new(76),
            crate::ids::PlacedSpawnId::new(),
            true,
        );
        let events: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        let (plan, repairs) =
            classify_placed_spawn_recovery(&definition.id, &[], std::slice::from_ref(&record))
                .expect("committed carrier produces one deterministic repair");
        assert_eq!(repairs.len(), 1);

        for (label, authority) in vec![
            ("missing owner", seed_mob_authority()),
            ("missing host", {
                let mut authority = seed_mob_authority();
                recover_owner_bridge_session_authority(
                    &mut authority,
                    &record.operation_owner_session_id,
                    false,
                    false,
                    "test_repair_owner",
                )
                .expect("recover owner only");
                authority
            }),
            ("mismatched owner", {
                let mut authority = seed_mob_authority();
                recover_owner_bridge_session_authority(
                    &mut authority,
                    &meerkat_core::SessionId::new(),
                    false,
                    false,
                    "test_repair_wrong_owner",
                )
                .expect("recover mismatched owner");
                authority
                    .apply_signal(mob_dsl::MobMachineSignal::RecoverHostBinding {
                        host_id: mob_dsl::HostId(record.host_id.to_string()),
                        pubkey: mob_dsl::PeerSigningKey([9; 32]),
                        endpoint: mob_dsl::PeerAddress("tcp://127.0.0.1:4100".to_string()),
                        epoch: 1,
                        binding_generation: record.host_binding_generation,
                        protocol_min: 4,
                        protocol_max: 4,
                        engine_version: "placed-recovery-test-engine".to_string(),
                        durable_sessions: true,
                        autonomous_members: true,
                        hard_cancel_member: true,
                        tracked_input_cancel: true,
                        memory_store: true,
                        mcp: true,
                        resolvable_providers: BTreeSet::from(["anthropic".to_string()]),
                        approval_forwarding: false,
                        live_endpoint: None,
                    })
                    .expect("recover carrier host");
                authority
            }),
        ]
        .into_boxed_slice()
        {
            validate_repaired_placed_spawn_machine_authority(&authority, &plan.entries[0])
                .expect_err(label);
            assert!(
                events
                    .replay_all()
                    .await
                    .expect("replay event store")
                    .is_empty(),
                "{label}: repair must not append before authority validation"
            );
        }
    }

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
                        objective_id: None,
                        phase: MobMemberKickoffPhase::Pending,
                        error: None,
                        updated_at: UNIX_EPOCH,
                    },
                },
            },
        ];
        let mut authority = seed_mob_authority();

        seed_mob_authority_sync_from_events(
            &mut authority,
            &events,
            &definition,
            false,
            &BTreeSet::new(),
        )
        .expect("durable kickoff replay should seed MobMachine");

        assert!(authority.state().member_kickoff_pending.contains(
            &crate::machines::mob_machine::AgentIdentity::from_domain(&identity,)
        ));
    }

    #[test]
    fn seeded_authority_reset_requires_exact_event_tuple_and_clears_old_kickoff() {
        let definition = MobDefinition::explicit("test-reset-mob");
        let identity = AgentIdentity::from("worker");
        for (label, phase, error) in [
            ("pending", MobMemberKickoffPhase::Pending, None),
            ("starting", MobMemberKickoffPhase::Starting, None),
            (
                "callback-pending",
                MobMemberKickoffPhase::CallbackPending,
                None,
            ),
            ("started", MobMemberKickoffPhase::Started, None),
            (
                "failed",
                MobMemberKickoffPhase::Failed,
                Some("old kickoff failure".to_string()),
            ),
            ("cancelled", MobMemberKickoffPhase::Cancelled, None),
        ] {
            let runtime_v0 = AgentRuntimeId::initial(identity.clone());
            let runtime_v1 = AgentRuntimeId::new(identity.clone(), Generation::new(1));
            let events = vec![
                MobEvent {
                    cursor: 1,
                    timestamp: Utc::now(),
                    mob_id: definition.id.clone(),
                    kind: MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                        identity.clone(),
                        Generation::INITIAL,
                        FenceToken::new(1),
                        runtime_v0,
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
                            objective_id: None,
                            phase,
                            error,
                            updated_at: UNIX_EPOCH,
                        },
                    },
                },
                MobEvent {
                    cursor: 3,
                    timestamp: Utc::now(),
                    mob_id: definition.id.clone(),
                    kind: MobEventKind::MemberReset {
                        agent_identity: identity.clone(),
                        previous_generation: Generation::INITIAL,
                        new_generation: Generation::new(1),
                        fence_token: FenceToken::new(2),
                        agent_runtime_id: runtime_v1.clone(),
                    },
                },
            ];
            let mut authority = seed_mob_authority();

            seed_mob_authority_sync_from_events(
                &mut authority,
                &events,
                &definition,
                false,
                &BTreeSet::new(),
            )
            .unwrap_or_else(|cause| panic!("{label}: valid reset replay failed: {cause}"));

            let dsl_identity = crate::machines::mob_machine::AgentIdentity::from_domain(&identity);
            assert_eq!(
                authority.state().identity_to_runtime.get(&dsl_identity),
                Some(&crate::machines::mob_machine::AgentRuntimeId::from_domain(
                    &runtime_v1
                )),
                "{label}"
            );
            assert_eq!(
                authority
                    .state()
                    .identity_runtime_generations
                    .get(&dsl_identity),
                Some(&crate::machines::mob_machine::Generation(1)),
                "{label}"
            );
            assert_eq!(
                authority
                    .state()
                    .identity_runtime_fence_tokens
                    .get(&dsl_identity),
                Some(&crate::machines::mob_machine::FenceToken(2)),
                "{label}"
            );
            assert!(
                !authority
                    .state()
                    .member_kickoff_pending
                    .contains(&dsl_identity)
                    && !authority
                        .state()
                        .member_kickoff_starting
                        .contains(&dsl_identity)
                    && !authority
                        .state()
                        .member_kickoff_callback_pending
                        .contains(&dsl_identity)
                    && !authority
                        .state()
                        .member_kickoff_started
                        .contains(&dsl_identity)
                    && !authority
                        .state()
                        .member_kickoff_failed
                        .contains(&dsl_identity)
                    && !authority
                        .state()
                        .member_kickoff_cancelled
                        .contains(&dsl_identity)
                    && !authority
                        .state()
                        .member_kickoff_error
                        .contains_key(&dsl_identity),
                "{label}: reset replay must clear every prior kickoff fact"
            );
        }
    }

    #[test]
    fn seeded_authority_rejects_malformed_member_reset_events_before_machine_replay() {
        let definition = MobDefinition::explicit("test-malformed-reset-mob");
        let identity = AgentIdentity::from("worker");
        let other_identity = AgentIdentity::from("other-worker");
        let cases = [
            (
                "identity mismatch",
                identity.clone(),
                Generation::INITIAL,
                Generation::new(1),
                AgentRuntimeId::new(other_identity, Generation::new(1)),
                "does not match runtime identity",
            ),
            (
                "runtime generation mismatch",
                identity.clone(),
                Generation::INITIAL,
                Generation::new(1),
                AgentRuntimeId::new(identity.clone(), Generation::new(2)),
                "does not match runtime generation",
            ),
            (
                "equal generation",
                identity.clone(),
                Generation::INITIAL,
                Generation::INITIAL,
                AgentRuntimeId::initial(identity.clone()),
                "is not the exact successor",
            ),
            (
                "regressed generation",
                identity.clone(),
                Generation::new(1),
                Generation::INITIAL,
                AgentRuntimeId::initial(identity.clone()),
                "is not the exact successor",
            ),
            (
                "skipped generation",
                identity.clone(),
                Generation::INITIAL,
                Generation::new(2),
                AgentRuntimeId::new(identity.clone(), Generation::new(2)),
                "is not the exact successor",
            ),
            (
                "generation overflow",
                identity.clone(),
                Generation::new(u64::MAX),
                Generation::INITIAL,
                AgentRuntimeId::initial(identity.clone()),
                "previous generation overflow",
            ),
        ];

        for (label, event_identity, previous, next, runtime, expected) in cases {
            let events = vec![MobEvent {
                cursor: 1,
                timestamp: Utc::now(),
                mob_id: definition.id.clone(),
                kind: MobEventKind::MemberReset {
                    agent_identity: event_identity,
                    previous_generation: previous,
                    new_generation: next,
                    fence_token: FenceToken::new(2),
                    agent_runtime_id: runtime,
                },
            }];
            let mut authority = seed_mob_authority();
            let error = seed_mob_authority_sync_from_events(
                &mut authority,
                &events,
                &definition,
                false,
                &BTreeSet::new(),
            )
            .expect_err(label);
            assert!(
                error.to_string().contains(expected),
                "{label}: unexpected error: {error}"
            );
        }
    }

    fn host_revoke_event(
        mob_id: &MobId,
        cursor: u64,
        operation_id: &str,
        confirmed: bool,
    ) -> MobEvent {
        MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: mob_id.clone(),
            kind: if confirmed {
                MobEventKind::RemoteHostRevokeConfirmed {
                    operation_id: operation_id.to_string(),
                    host_id: "host-peer".to_string(),
                    epoch: 7,
                    binding_generation: 1,
                }
            } else {
                MobEventKind::RemoteHostRevokeStarted {
                    operation_id: operation_id.to_string(),
                    host_id: "host-peer".to_string(),
                    epoch: 7,
                    binding_generation: 1,
                }
            },
        }
    }

    #[tokio::test]
    async fn recovered_host_revoke_retries_before_and_after_proof_then_closes_after_local_terminal()
    {
        let mob_id = MobId::from("recovered-host-revoke");
        let operation_id = meerkat_core::time_compat::new_uuid_v7().to_string();
        let metadata = Arc::new(InMemoryMobRuntimeMetadataStore::new());
        let record = crate::store::sample_mob_host_authority_record("host-peer", 7);
        let authority = crate::store::mob_host_authority_persistence_authority_for_record(&record)
            .expect("host authority witness");
        metadata
            .put_mob_host_authority(&mob_id, &record, &authority)
            .await
            .expect("seed host authority");
        let events: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        let started = host_revoke_event(&mob_id, 1, &operation_id, false);

        assert_eq!(
            MobBuilder::prepare_recovered_host_revocations(
                metadata.clone(),
                events.clone(),
                &mob_id,
                std::slice::from_ref(&started),
            )
            .await
            .expect("crash before remote proof remains retryable"),
            vec!["host-peer".to_string()],
        );

        let confirmed = host_revoke_event(&mob_id, 2, &operation_id, true);
        assert_eq!(
            MobBuilder::prepare_recovered_host_revocations(
                metadata.clone(),
                events.clone(),
                &mob_id,
                &[started.clone(), confirmed.clone()],
            )
            .await
            .expect("crash after remote proof still retries local convergence"),
            vec!["host-peer".to_string()],
        );

        let empty_metadata: Arc<dyn MobRuntimeMetadataStore> =
            Arc::new(InMemoryMobRuntimeMetadataStore::new());
        assert!(
            MobBuilder::prepare_recovered_host_revocations(
                empty_metadata,
                events.clone(),
                &mob_id,
                &[started, confirmed],
            )
            .await
            .expect("missing authority after proof means local terminal holds")
            .is_empty()
        );
        assert!(
            events
                .replay_all()
                .await
                .expect("completion events")
                .iter()
                .any(|event| matches!(
                    &event.kind,
                    MobEventKind::RemoteHostRevokeCompleted {
                        operation_id: completed,
                        host_id,
                        epoch: 7,
                        binding_generation: 1,
                    } if completed == &operation_id && host_id == "host-peer"
                )),
            "recovery closes the exact op before a later same-epoch rebind can be exposed"
        );
    }

    fn terminal_preserving_respawn_gap_events(definition: &MobDefinition) -> Vec<MobEvent> {
        let retiring = AgentIdentity::from("alpha");
        let peer = AgentIdentity::from("beta");
        let generation = Generation::INITIAL;
        let retiring_runtime = AgentRuntimeId::new(retiring.clone(), generation);
        let event = |cursor, kind| MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: definition.id.clone(),
            kind,
        };
        vec![
            event(
                1,
                MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                    retiring.clone(),
                    generation,
                    FenceToken::new(7),
                    retiring_runtime.clone(),
                    ProfileName::from("worker"),
                )),
            ),
            event(
                2,
                MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                    peer.clone(),
                    generation,
                    FenceToken::new(9),
                    AgentRuntimeId::new(peer.clone(), generation),
                    ProfileName::from("worker"),
                )),
            ),
            event(
                3,
                MobEventKind::MembersWired {
                    a: retiring.clone(),
                    b: peer,
                },
            ),
            event(
                4,
                MobEventKind::MemberRetirementStarted {
                    agent_identity: retiring.clone(),
                    agent_runtime_id: retiring_runtime,
                    generation,
                    role: ProfileName::from("worker"),
                    releasing: None,
                    session_id: None,
                    retiring_peer_endpoint: None,
                    preserve_machine_topology: true,
                },
            ),
            event(
                5,
                MobEventKind::MemberRetired {
                    agent_identity: retiring,
                    generation,
                    role: ProfileName::from("worker"),
                },
            ),
        ]
    }

    async fn persist_event_kinds(
        store: &InMemoryMobEventStore,
        mob_id: &MobId,
        events: &[MobEvent],
    ) -> Vec<MobEvent> {
        store
            .append_batch(
                events
                    .iter()
                    .map(|event| NewMobEvent {
                        mob_id: mob_id.clone(),
                        timestamp: None,
                        kind: event.kind.clone(),
                    })
                    .collect(),
            )
            .await
            .expect("persist test mob events")
    }

    #[test]
    fn respawn_topology_ledger_rejects_conflicting_starts_and_markers() {
        let definition = MobDefinition::explicit("respawn-topology-ledger-conflicts");
        let base = terminal_preserving_respawn_gap_events(&definition);
        let identity = AgentIdentity::from("alpha");
        let runtime_id = AgentRuntimeId::initial(identity.clone());

        let mut conflicting_starts = base[..4].to_vec();
        conflicting_starts.push(MobEvent {
            cursor: 5,
            timestamp: Utc::now(),
            mob_id: definition.id.clone(),
            kind: MobEventKind::MemberRetirementStarted {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                generation: Generation::INITIAL,
                role: ProfileName::from("worker"),
                releasing: None,
                session_id: None,
                retiring_peer_endpoint: None,
                preserve_machine_topology: false,
            },
        });
        let start_error = respawn_topology_ledger(&conflicting_starts)
            .err()
            .expect("same-generation retirement starts may not change preservation intent");
        assert!(
            start_error
                .to_string()
                .contains("conflicting MemberRetirementStarted carriers"),
            "unexpected conflicting-start error: {start_error}"
        );

        let mut conflicting_markers = base[..4].to_vec();
        conflicting_markers.push(MobEvent {
            cursor: 5,
            timestamp: Utc::now(),
            mob_id: definition.id.clone(),
            kind: MobEventKind::RespawnTopologyAbandoned {
                agent_identity: identity.clone(),
                generation: Generation::INITIAL,
                agent_runtime_id: Some(runtime_id.clone()),
                fence_token: Some(FenceToken::new(7)),
            },
        });
        conflicting_markers.push(MobEvent {
            cursor: 6,
            timestamp: Utc::now(),
            mob_id: definition.id.clone(),
            kind: MobEventKind::RespawnTopologyAbandoned {
                agent_identity: identity,
                generation: Generation::INITIAL,
                agent_runtime_id: Some(runtime_id),
                fence_token: Some(FenceToken::new(8)),
            },
        });
        let marker_error = respawn_topology_ledger(&conflicting_markers)
            .err()
            .expect("marker fence must match the exact spawned incarnation");
        assert!(
            marker_error
                .to_string()
                .contains("conflicts with its spawned incarnation"),
            "unexpected conflicting-marker error: {marker_error}"
        );
    }

    #[tokio::test]
    async fn recovered_respawn_abandonment_append_is_once_and_reset_placed_scoped() {
        let definition = MobDefinition::explicit("respawn-abandonment-append-once");
        let source = terminal_preserving_respawn_gap_events(&definition);
        let store = InMemoryMobEventStore::new();
        let mut events = persist_event_kinds(&store, &definition.id, &source).await;

        ensure_recovered_respawn_topology_abandonments(
            &store,
            &definition.id,
            &mut events,
            &BTreeSet::new(),
        )
        .await
        .expect("first recovery appends exact abandonment marker");
        ensure_recovered_respawn_topology_abandonments(
            &store,
            &definition.id,
            &mut events,
            &BTreeSet::new(),
        )
        .await
        .expect("second recovery observes the already durable marker");
        let stored = store.replay_all().await.expect("replay appended marker");
        let markers = stored
            .iter()
            .filter(|event| matches!(&event.kind, MobEventKind::RespawnTopologyAbandoned { .. }))
            .collect::<Vec<_>>();
        assert_eq!(markers.len(), 1, "recovery must append exactly one marker");
        assert!(matches!(
            &markers[0].kind,
            MobEventKind::RespawnTopologyAbandoned {
                agent_identity,
                generation,
                agent_runtime_id: Some(runtime_id),
                fence_token: Some(fence_token),
            } if agent_identity.as_str() == "alpha"
                && *generation == Generation::INITIAL
                && runtime_id == &AgentRuntimeId::initial(AgentIdentity::from("alpha"))
                && *fence_token == FenceToken::new(7)
        ));

        let placed_definition = MobDefinition::explicit("respawn-abandonment-placed-successor");
        let placed_source = terminal_preserving_respawn_gap_events(&placed_definition);
        let placed_store = InMemoryMobEventStore::new();
        let mut placed_events =
            persist_event_kinds(&placed_store, &placed_definition.id, &placed_source).await;
        let committed_successor =
            BTreeSet::from([("alpha".to_string(), 1, crate::ids::PlacedSpawnId::new())]);
        ensure_recovered_respawn_topology_abandonments(
            &placed_store,
            &placed_definition.id,
            &mut placed_events,
            &committed_successor,
        )
        .await
        .expect("current committed placed successor suppresses abandonment");
        assert!(
            placed_store
                .replay_all()
                .await
                .expect("replay placed successor scope")
                .iter()
                .all(|event| !matches!(&event.kind, MobEventKind::RespawnTopologyAbandoned { .. })),
            "a current committed placed successor must not receive a stale abandonment marker"
        );

        let reset_definition = MobDefinition::explicit("respawn-abandonment-reset-scope");
        let mut reset_source = terminal_preserving_respawn_gap_events(&reset_definition);
        reset_source.push(MobEvent {
            cursor: 6,
            timestamp: Utc::now(),
            mob_id: reset_definition.id.clone(),
            kind: MobEventKind::MobReset,
        });
        let reset_store = InMemoryMobEventStore::new();
        let mut reset_events =
            persist_event_kinds(&reset_store, &reset_definition.id, &reset_source).await;
        ensure_recovered_respawn_topology_abandonments(
            &reset_store,
            &reset_definition.id,
            &mut reset_events,
            &BTreeSet::new(),
        )
        .await
        .expect("pre-reset preservation gap is outside the active epoch");
        assert!(
            reset_store
                .replay_all()
                .await
                .expect("replay reset scope")
                .iter()
                .all(|event| !matches!(&event.kind, MobEventKind::RespawnTopologyAbandoned { .. })),
            "recovery must not append a marker for a pre-reset incarnation"
        );
    }

    #[tokio::test]
    async fn respawn_abandonment_survives_two_resumes_without_same_id_edge_resurrection() {
        let definition = MobDefinition::explicit("respawn-abandonment-two-resume");
        let source = terminal_preserving_respawn_gap_events(&definition);
        let store = InMemoryMobEventStore::new();
        let mut events = persist_event_kinds(&store, &definition.id, &source).await;
        ensure_recovered_respawn_topology_abandonments(
            &store,
            &definition.id,
            &mut events,
            &BTreeSet::new(),
        )
        .await
        .expect("first resume appends terminal abandonment");

        let edge = dsl_wiring_edge(&AgentIdentity::from("alpha"), &AgentIdentity::from("beta"));
        let mut first_resume = seed_mob_authority();
        seed_mob_authority_sync_from_events(
            &mut first_resume,
            &events,
            &definition,
            false,
            &BTreeSet::new(),
        )
        .expect("seed first resumed authority");
        recover_pending_member_retirements(&mut first_resume, &events)
            .expect("converge first resumed topology");
        assert!(
            !first_resume.state().wiring_edges.contains(&edge),
            "first resume must prune the abandoned generation-zero edge"
        );

        let successor_identity = AgentIdentity::from("alpha");
        let successor_generation = Generation::new(1);
        let successor = store
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                    successor_identity.clone(),
                    successor_generation,
                    FenceToken::new(8),
                    AgentRuntimeId::new(successor_identity.clone(), successor_generation),
                    ProfileName::from("worker"),
                )),
            })
            .await
            .expect("append a later fresh same-id successor");
        events.push(successor);
        ensure_recovered_respawn_topology_abandonments(
            &store,
            &definition.id,
            &mut events,
            &BTreeSet::new(),
        )
        .await
        .expect("second resume keeps the exact marker idempotent");

        let mut second_resume = seed_mob_authority();
        seed_mob_authority_sync_from_events(
            &mut second_resume,
            &events,
            &definition,
            false,
            &BTreeSet::new(),
        )
        .expect("seed second resumed authority");
        recover_pending_member_retirements(&mut second_resume, &events)
            .expect("converge second resumed topology");
        let successor_dsl = mob_dsl::AgentIdentity::from_domain(&successor_identity);
        assert_eq!(
            second_resume
                .state()
                .identity_runtime_generations
                .get(&successor_dsl),
            Some(&mob_dsl::Generation::from_domain(successor_generation))
        );
        assert!(
            !second_resume.state().wiring_edges.contains(&edge),
            "the historical edge must not resurrect beneath a later fresh same-id successor"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    &event.kind,
                    MobEventKind::RespawnTopologyAbandoned { .. }
                ))
                .count(),
            1,
            "two resumes must retain one canonical abandonment marker"
        );
    }

    #[test]
    fn recovered_respawn_topology_requires_a_durable_successor() {
        fn recover_case(
            preserve_machine_topology: bool,
            include_successor: bool,
        ) -> mob_dsl::MobMachineAuthority {
            let definition = MobDefinition::explicit("respawn-topology-replay");
            let retiring = AgentIdentity::from("alpha");
            let peer = AgentIdentity::from("beta");
            let generation_zero = Generation::INITIAL;
            let generation_one = generation_zero.next().expect("successor generation");
            let retiring_runtime = AgentRuntimeId::new(retiring.clone(), generation_zero);
            let peer_runtime = AgentRuntimeId::new(peer.clone(), generation_zero);
            let event = |cursor, kind| MobEvent {
                cursor,
                timestamp: Utc::now(),
                mob_id: definition.id.clone(),
                kind,
            };
            let mut events = vec![
                event(
                    1,
                    MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                        retiring.clone(),
                        generation_zero,
                        FenceToken::new(1),
                        retiring_runtime.clone(),
                        ProfileName::from("worker"),
                    )),
                ),
                event(
                    2,
                    MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                        peer.clone(),
                        generation_zero,
                        FenceToken::new(2),
                        peer_runtime,
                        ProfileName::from("worker"),
                    )),
                ),
                event(
                    3,
                    MobEventKind::MembersWired {
                        a: retiring.clone(),
                        b: peer,
                    },
                ),
                event(
                    4,
                    MobEventKind::MemberRetirementStarted {
                        agent_identity: retiring.clone(),
                        agent_runtime_id: retiring_runtime,
                        generation: generation_zero,
                        role: ProfileName::from("worker"),
                        releasing: None,
                        session_id: None,
                        retiring_peer_endpoint: None,
                        preserve_machine_topology,
                    },
                ),
                event(
                    5,
                    MobEventKind::MemberRetired {
                        agent_identity: retiring.clone(),
                        generation: generation_zero,
                        role: ProfileName::from("worker"),
                    },
                ),
            ];
            if include_successor {
                events.push(event(
                    6,
                    MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
                        retiring.clone(),
                        generation_one,
                        FenceToken::new(3),
                        AgentRuntimeId::new(retiring, generation_one),
                        ProfileName::from("worker"),
                    )),
                ));
            }

            let mut authority = seed_mob_authority();
            seed_mob_authority_sync_from_events(
                &mut authority,
                &events,
                &definition,
                false,
                &BTreeSet::new(),
            )
            .expect("seed replay authority");
            recover_pending_member_retirements(&mut authority, &events)
                .expect("converge replay topology");
            authority
        }

        let alpha = mob_dsl::AgentIdentity::from("alpha");
        let beta = mob_dsl::AgentIdentity::from("beta");
        let edge = mob_dsl::WiringEdge::new(alpha.clone(), beta);

        let preserved_gap = recover_case(true, false);
        assert!(!preserved_gap.state().wiring_edges.contains(&edge));
        assert!(
            preserved_gap.state().pending_respawn_topology.is_empty(),
            "a retirement marker alone is not a durable replacement operation"
        );

        let preserved_successor = recover_case(true, true);
        assert!(preserved_successor.state().wiring_edges.contains(&edge));
        assert!(
            !preserved_successor
                .state()
                .pending_respawn_topology
                .contains(&alpha),
            "successor membership must consume the topology hold"
        );

        let ordinary_gap = recover_case(false, false);
        assert!(!ordinary_gap.state().wiring_edges.contains(&edge));
        assert!(ordinary_gap.state().pending_respawn_topology.is_empty());

        let ordinary_successor = recover_case(false, true);
        assert!(
            !ordinary_successor.state().wiring_edges.contains(&edge),
            "a later fresh spawn must not resurrect ordinary-retirement topology"
        );
        assert!(
            ordinary_successor
                .state()
                .pending_respawn_topology
                .is_empty()
        );
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
                    preserve_machine_topology: false,
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

        seed_mob_authority_sync_from_events(
            &mut authority,
            &events,
            &definition,
            false,
            &BTreeSet::new(),
        )
        .expect("durable retirement replay should seed base MobMachine facts");
        recover_pending_member_retirements(&mut authority, &events)
            .expect("durable retirement replay should restore pending checkpoints");

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
                disposal: crate::machines::mob_machine::MemberSessionDisposal::Archived,
                preserve_machine_topology: false,
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
                preserve_machine_topology: false,
            },
            "test_recovered_remote_runtime_revoked_terminal_clear",
        )
        .expect("revoked remote terminal proof should clear the runtime checkpoint");
        assert!(authority.state().remote_runtime_retired_ids.is_empty());
    }
}
