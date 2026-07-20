use super::flow_frame_engine::FlowFrameLoopStorePlan;
use super::terminalization::{TerminalizationOutcome, TerminalizationTarget};
use super::*;
use crate::machines::mob_machine as mob_dsl;
use crate::run::{MobMachineFlowRunCommand, MobRun, flow_run};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

// ---------------------------------------------------------------------------
// MobState
// ---------------------------------------------------------------------------

/// Lifecycle state of a mob. Projected from the DSL authority on demand —
/// no shadow truth (see dogma #1, #13, #17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MobState {
    Creating = 0,
    Running = 1,
    Stopped = 2,
    Completed = 3,
    Destroyed = 4,
}

impl MobState {
    /// Human-readable name for the state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "Creating",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Completed => "Completed",
            Self::Destroyed => "Destroyed",
        }
    }
}

impl std::fmt::Display for MobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Diagnostic snapshots (DSL-projected, shell-owned)
// ---------------------------------------------------------------------------

/// Observable snapshot of mob orchestrator-facing state.
///
/// Projected from the MobMachine DSL state plus shell-owned metadata that is
/// not tracked by the DSL authority (`topology_revision`, `supervisor_active`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobOrchestratorSnapshot {
    pub phase: MobState,
    pub coordinator_bound: bool,
    pub pending_spawn_count: u32,
    pub active_flow_count: u32,
    pub topology_revision: u32,
    pub supervisor_active: bool,
}

impl Default for MobOrchestratorSnapshot {
    fn default() -> Self {
        Self {
            phase: MobState::Creating,
            coordinator_bound: false,
            pending_spawn_count: 0,
            active_flow_count: 0,
            topology_revision: 0,
            supervisor_active: false,
        }
    }
}

/// Observable snapshot of mob lifecycle-facing state.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MobLifecycleSnapshot {
    pub phase: MobState,
    pub active_run_count: u32,
    pub cleanup_pending: bool,
}

#[cfg(test)]
impl Default for MobLifecycleSnapshot {
    fn default() -> Self {
        Self {
            phase: MobState::Running,
            active_run_count: 0,
            cleanup_pending: false,
        }
    }
}

/// Test-only projection of the Phase 5G / T2 DSL fields. Cloned from
/// `MobMachineAuthority.state` inside the actor so the shell sees the DSL
/// authority directly (dogma #1, #13) with no shadow truth on the handle.
/// Types mirror the DSL-scoped types declared by `machines::mob_machine` —
/// they are distinct from the public `crate::ids::*` types.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(crate) struct MobDslT2Snapshot {
    pub destroy_admitted: bool,
    pub flow_authority_schema_version: u64,
    pub owner_bridge_session_id: Option<crate::machines::mob_machine::SessionId>,
    pub owner_bridge_destroy_on_archive: bool,
    pub implicit_delegation_mob: bool,
    pub supervisor_authority_peer_id: Option<crate::machines::mob_machine::PeerId>,
    pub supervisor_authority_signing_key: Option<crate::machines::mob_machine::PeerSigningKey>,
    pub supervisor_authority_epoch: Option<u64>,
    pub supervisor_authority_protocol_version:
        Option<crate::machines::mob_machine::SupervisorProtocolVersion>,
    pub supervisor_pending_authority_peer_id: Option<crate::machines::mob_machine::PeerId>,
    pub supervisor_pending_authority_signing_key:
        Option<crate::machines::mob_machine::PeerSigningKey>,
    pub supervisor_pending_authority_epoch: Option<u64>,
    pub supervisor_pending_authority_protocol_version:
        Option<crate::machines::mob_machine::SupervisorProtocolVersion>,
    pub supervisor_pending_authority_operation_id: Option<String>,
    pub supervisor_pending_authority_member_target_names:
        std::collections::BTreeMap<crate::machines::mob_machine::PeerId, String>,
    pub supervisor_pending_authority_member_target_addresses:
        std::collections::BTreeMap<crate::machines::mob_machine::PeerId, String>,
    pub supervisor_pending_authority_accepted_peer_ids:
        std::collections::BTreeSet<crate::machines::mob_machine::PeerId>,
    // Dogma row R044: machine-owned trust-install-before-terminality
    // obligation window for supervisor-bridge recipients.
    pub pending_recipient_trust: std::collections::BTreeSet<crate::machines::mob_machine::PeerId>,
    // Exact host-binding authority and retained revoke/replacement state.
    // These are projected from MobMachine directly so runtime parity never
    // substitutes an empty shell-side shadow for a live host generation.
    pub host_binding_generations:
        std::collections::BTreeMap<crate::machines::mob_machine::HostId, u64>,
    pub host_binding_generation_highwater:
        std::collections::BTreeMap<crate::machines::mob_machine::HostId, u64>,
    pub confirmed_host_binding_revocations:
        std::collections::BTreeSet<crate::machines::mob_machine::HostBindingGenerationTombstone>,
    pub replacement_host_bind_endpoints: std::collections::BTreeMap<
        crate::machines::mob_machine::HostId,
        crate::machines::mob_machine::PeerAddress,
    >,
    pub replacement_host_binding_generations:
        std::collections::BTreeMap<crate::machines::mob_machine::HostId, u64>,
    pub member_state_markers: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentRuntimeId,
        crate::machines::mob_machine::MobMemberState,
    >,
    pub wiring_edges: std::collections::BTreeSet<crate::machines::mob_machine::WiringEdge>,
    pub external_peer_edges:
        std::collections::BTreeSet<crate::machines::mob_machine::ExternalPeerEdge>,
    pub external_peer_edges_by_key: std::collections::BTreeMap<
        crate::machines::mob_machine::ExternalPeerKey,
        crate::machines::mob_machine::ExternalPeerEdge,
    >,
    pub pending_respawn_topology:
        std::collections::BTreeSet<crate::machines::mob_machine::AgentIdentity>,
    pub abandoned_respawn_topology: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::Generation,
    >,
    pub identity_to_runtime: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::AgentRuntimeId,
    >,
    pub identity_runtime_generations: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::Generation,
    >,
    pub identity_runtime_fence_tokens: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::FenceToken,
    >,
    pub member_profile_names:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub member_runtime_modes: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::SpawnPolicyRuntimeMode,
    >,
    pub member_peer_ids: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::PeerId,
    >,
    pub member_peer_endpoints: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::MemberPeerEndpoint,
    >,
    pub member_prior_peer_endpoints: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        std::collections::BTreeSet<crate::machines::mob_machine::MemberPeerEndpoint>,
    >,
    pub member_restore_failures:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub member_restore_failure_codes:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub runtime_retire_refusal_codes:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentRuntimeId, String>,
    pub runtime_retire_refusal_reasons:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentRuntimeId, String>,
    pub runtime_retire_pending_sessions: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentRuntimeId,
        crate::machines::mob_machine::SessionId,
    >,
    pub remote_runtime_retired_ids:
        std::collections::BTreeSet<crate::machines::mob_machine::AgentRuntimeId>,
    pub remote_supervisor_revoked_ids:
        std::collections::BTreeSet<crate::machines::mob_machine::AgentRuntimeId>,
    // #37: machine-owned revival obligation for members whose live
    // materialization is gone while a durable snapshot remains.
    pub member_revival_pending:
        std::collections::BTreeSet<crate::machines::mob_machine::AgentIdentity>,
    pub member_kickoff_objective_ids:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub objective_owner_ids:
        std::collections::BTreeMap<String, crate::machines::mob_machine::AgentIdentity>,
    pub objective_outcomes: std::collections::BTreeMap<String, String>,
    pub concluded_objective_ids: std::collections::BTreeSet<String>,
    pub member_run_open:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, bool>,
    pub member_in_flight_work:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, u64>,
    pub member_progress_tokens:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub member_last_observed_at_ms:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, u64>,
    pub member_last_progress_at_ms:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, u64>,
    pub member_last_progress_event: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::MemberProgressEventKind,
    >,
    pub member_health_class: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::MemberHealthClass,
    >,
    // W3-H-1: canonical identity→bridge-session binding map, projected from
    // `MobMachineAuthority.state.member_session_bindings`. Used by the
    // runtime-parity snapshot to expose the DSL's realtime binding map to
    // integration tests.
    pub member_session_bindings: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::SessionId,
    >,
    // Machine-driven spawn-exec ladder phase per in-flight identity, projected
    // from `MobMachineAuthority.state.spawn_exec_phase`. Exposed so the runtime
    // parity field evaluator covers the formal `spawn_exec_phase` state field.
    pub spawn_exec_phase: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::SpawnExecPhase,
    >,
    pub pending_spawn_sessions: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::SessionId,
    >,
    pub pending_session_ingress_detach_runtime_ids:
        std::collections::BTreeSet<crate::machines::mob_machine::AgentRuntimeId>,
    pub topology_epoch: u64,
    pub spawn_policy_enabled: bool,
    pub spawn_policy_revision: u64,
    pub spawn_policy_resolution_revision:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, u64>,
    pub spawn_policy_resolution_profiles:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub spawn_policy_resolution_runtime_modes: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        Option<crate::machines::mob_machine::SpawnPolicyRuntimeMode>,
    >,
    pub spawn_policy_resolution_absent:
        std::collections::BTreeSet<crate::machines::mob_machine::AgentIdentity>,
    pub spawn_profile_authority_profile_names:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub spawn_profile_authority_models:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub spawn_profile_authority_material_digests:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub spawn_profile_authority_tool_config_digests:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub spawn_profile_authority_skills_digests:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub spawn_profile_authority_provider_params_digests:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, Option<String>>,
    pub spawn_profile_authority_output_schema_digests:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, Option<String>>,
    pub spawn_profile_authority_external_addressable:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, bool>,
    // Row #320: machine-owned orphan budget for hard flow-turn timeouts.
    pub orphan_budget: u64,
    // Row #293: machine-owned default topology policy applied to unmatched edges.
    pub topology_default_policy: crate::machines::mob_machine::PolicyDecision,
    // Row #314: machine-owned per-member external rebind capability.
    pub external_member_rebind_capability: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::ExternalMemberRebindCapability,
    >,
    // Row #351: machine-owned reconcile membership sets.
    pub desired_members: std::collections::BTreeSet<crate::machines::mob_machine::AgentIdentity>,
    pub members_to_spawn: std::collections::BTreeSet<crate::machines::mob_machine::AgentIdentity>,
    pub members_to_retire: std::collections::BTreeSet<crate::machines::mob_machine::AgentIdentity>,
    pub pending_placed_spawn_ids: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::PlacedSpawnId,
    >,
    pub pending_placed_spawn_generations: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::Generation,
    >,
    pub pending_placed_spawn_fence_tokens: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::FenceToken,
    >,
    pub pending_placed_spawn_hosts: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::HostId,
    >,
    pub pending_autonomous_placed_spawns:
        std::collections::BTreeSet<crate::machines::mob_machine::AgentIdentity>,
    pub pending_placed_spawn_host_binding_generations:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, u64>,
    pub pending_placed_spawn_spec_digests:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub pending_placed_spawn_provision_operation_ids:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub pending_placed_spawn_operation_owner_session_ids: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::SessionId,
    >,
    pub current_placed_spawn_ids: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::PlacedSpawnId,
    >,
    pub current_placed_spawn_host_binding_generations:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, u64>,
    pub current_placed_spawn_provision_operation_ids:
        std::collections::BTreeMap<crate::machines::mob_machine::AgentIdentity, String>,
    pub current_placed_spawn_operation_owner_session_ids: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::SessionId,
    >,
    pub pending_placed_carrier_cleanup:
        std::collections::BTreeSet<crate::machines::mob_machine::PlacedCarrierCleanupObligation>,
    pub remote_turn_dispatch_sequence: u64,
    pub committed_remote_turn_outcomes:
        std::collections::BTreeSet<crate::machines::mob_machine::RemoteTurnObligation>,
    pub resolved_remote_turn_outcomes:
        std::collections::BTreeSet<crate::machines::mob_machine::RemoteTurnObligation>,
    pub pending_placed_kickoff_outcomes:
        std::collections::BTreeSet<crate::machines::mob_machine::PlacedKickoffObligation>,
    pub resolved_placed_kickoff_outcomes:
        std::collections::BTreeSet<crate::machines::mob_machine::PlacedKickoffObligation>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MobStartupKickoffSnapshot {
    /// Kickoffs whose one-shot completion handle has not reached a terminal.
    /// `CallbackPending` remains projected on the member but is settled here.
    pub pending_kickoff_member_ids: std::collections::BTreeSet<String>,
    pub ready_runtime_ids: std::collections::BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MobMemberMachineProjection {
    pub runtime_id: Option<crate::machines::mob_machine::AgentRuntimeId>,
    pub state_marker: Option<crate::machines::mob_machine::MobMemberState>,
    pub live_runtime: bool,
    pub bound_session_id: Option<crate::machines::mob_machine::SessionId>,
}

/// Heap-allocated payload for `MobCommand::SubmitWork`. Boxing keeps the
/// command-channel variant width bounded by the pointer, not by the full
/// `ContentInput` + render metadata footprint.
pub(super) struct SubmitWorkPayload {
    pub runtime_id: AgentRuntimeId,
    pub fence_token: FenceToken,
    pub work_ref: WorkRef,
    pub content: ContentInput,
    pub origin: WorkOrigin,
    /// Host-attached injected context riding with the work content. Shell
    /// dispatch carrier only — the MobMachine DSL admits identity facts
    /// (runtime id, fence, work id, origin), never content payloads.
    pub injected_context: Vec<ContentInput>,
    /// Host-supplied interaction identity riding with the work content.
    /// Shell dispatch carrier only (content-adjacent metadata); stamped into
    /// the turn's transcript identity at delivery so the committed transcript
    /// joins the host's live frames.
    pub interaction_id: Option<meerkat_core::interaction::InteractionId>,
    /// Durable objective causality riding with this work item.
    pub objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    pub handling_mode: meerkat_core::types::HandlingMode,
    /// Typed runtime/session semantics for this turn. The actor merges the
    /// WorkSpec-owned transcript identity into this carrier before dispatch.
    pub turn_metadata: Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
    /// Optional live event stream for this specific member turn.
    pub event_tx:
        Option<tokio::sync::mpsc::Sender<meerkat_core::EventEnvelope<meerkat_core::AgentEvent>>>,
    /// Optional committed-completion result channel for a tracked turn.
    pub completion_tx: Option<oneshot::Sender<Result<(), MobError>>>,
    /// Executor-bound acknowledgement for a host-requested LLM identity.
    pub llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
    pub ack_mode: crate::mob_machine::SubmitWorkAckMode,
}

// ---------------------------------------------------------------------------
// MobCommand
// ---------------------------------------------------------------------------

/// Exact volatile correlation for one detached orphan-release operation.
///
/// This is mechanical in-flight state, not lifecycle truth: the host's
/// durable release receipt remains authoritative, and an actor restart drops
/// the set so the next status poll retries. The fields carry the exact
/// `ReleaseMember` idempotency authority plus the actor-local host binding
/// incarnation; the protocol forbids a different session/spec at the same
/// generation/fence tuple within one incarnation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct HostOrphanReleaseKey {
    pub(super) host_id: mob_dsl::HostId,
    /// Volatile actor-owned binding incarnation. This is deliberately
    /// independent from the supervisor authority epoch: revoke followed by a
    /// fresh bind may reuse that epoch while targeting a different host
    /// process/route.
    pub(super) binding_incarnation: u64,
    pub(super) agent_identity: mob_dsl::AgentIdentity,
    pub(super) generation: mob_dsl::Generation,
    pub(super) fence_token: mob_dsl::FenceToken,
}

/// Internal delivery handshake for one successful member-live Open. The
/// actor retains exact cleanup custody until the handle acknowledges receipt;
/// dropping this value anywhere between the oneshot send and the public
/// method return closes the ack sender and triggers exact channel cleanup.
pub(super) struct MemberLiveOpenDelivery {
    pub(super) open: super::bridge_protocol::LiveOpenResult,
    /// The handle sends a confirmation channel and waits for the actor to
    /// delete durable cleanup custody before it returns the public Open.
    pub(super) delivery_ack: oneshot::Sender<oneshot::Sender<Result<(), MobError>>>,
}

/// Commands sent from [`MobHandle`] to the [`MobActor`] for serialized processing.
pub(super) enum PlacedBehaviorCompletion {
    MemberHistory {
        result: Result<super::member_history_proxy::MemberHistoryPageDomain, MobError>,
        reply_tx:
            oneshot::Sender<Result<super::member_history_proxy::MemberHistoryPageDomain, MobError>>,
    },
    LiveClose {
        result: Result<super::bridge_protocol::LiveCloseStatus, MobError>,
        reply_tx: oneshot::Sender<Result<super::bridge_protocol::LiveCloseStatus, MobError>>,
    },
    LiveStatus {
        result: Result<super::member_live_proxy::MemberLiveStatusDomain, MobError>,
        reply_tx:
            oneshot::Sender<Result<super::member_live_proxy::MemberLiveStatusDomain, MobError>>,
    },
}

pub(super) enum MobCommand {
    Spawn {
        spec: Box<super::handle::SpawnMemberSpec>,
        spawn_source: super::handle::SpawnSource,
        owner_bridge_session_id: Option<SessionId>,
        ops_registry: Option<Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>>,
        reply_tx: oneshot::Sender<Result<super::handle::MemberSpawnReceipt, MobError>>,
    },
    SpawnProvisioned {
        spawn_ticket: u64,
        result: Result<super::handle::MemberSpawnReceipt, MobError>,
    },
    /// Internal trigger (multi-host §9, W-D.2): a delivery to a PLACED
    /// member failed on the bridge, or a `HostStatus` sweep reported it
    /// unhealthy/missing. The actor feeds the raw observation to the
    /// machine's `ClassifyMemberLiveMaterialization` ladder; there is no
    /// public caller and no reply — the outcome lands in machine state
    /// (`member_revival_pending`/Broken) and the restore diagnostics.
    RevivePlacedMember {
        agent_identity: AgentIdentity,
        reason: String,
    },
    /// Completion of one detached periodic `HostStatus` observation.
    ///
    /// The network wait never runs on the serialized actor task. The
    /// `binding_epoch` is checked again when this command is consumed so a
    /// late response from a revoked/rebound host cannot reconcile current
    /// machine state.
    HostStatusPollCompleted {
        host_id: String,
        binding_epoch: u64,
        binding_generation: u64,
        binding_incarnation: u64,
        result: Result<super::bridge_protocol::BridgeHostStatusResponse, crate::MobError>,
    },
    /// A successful, authenticated member-events page carrying the exact
    /// member-host boot token. The actor revalidates the full residency and
    /// converges volatile route installs before acknowledging the pump, so
    /// event-pump reachability cannot race host restart recovery.
    HostRuntimeIncarnationObserved {
        expected_member: super::bridge_protocol::BridgeMemberIncarnation,
        runtime_incarnation: super::bridge_protocol::BridgeHostRuntimeIncarnation,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Typed completion of one exact detached orphan release. The actor keeps
    /// the key reserved until this command is absorbed, suppressing overlap
    /// from periodic and rebind-triggered status observations.
    HostOrphanReleaseCompleted {
        key: HostOrphanReleaseKey,
        result: Result<crate::machines::mob_machine::MemberSessionDisposal, crate::MobError>,
    },
    /// Completion of a detached placed-member behavior. The actor compares
    /// the captured exact host/member incarnation immediately before sending
    /// any success to the caller, linearizing response attribution against
    /// host revoke/rebind/promotion.
    PlacedBehaviorCompleted {
        agent_identity: AgentIdentity,
        expected_member: super::bridge_protocol::BridgeMemberIncarnation,
        completion: PlacedBehaviorCompletion,
    },
    Retire {
        agent_identity: AgentIdentity,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Respawn {
        agent_identity: AgentIdentity,
        initial_message: Option<ContentInput>,
        reply_tx: oneshot::Sender<
            Result<super::handle::MemberRespawnReceipt, super::handle::MobRespawnError>,
        >,
    },
    RetireAll {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Unified work-lane ingress: the MobMachine DSL decides work-origin
    /// legality (External vs Internal, addressability, live-runtime, phase),
    /// the shell observes the machine's `RequestRuntimeIngress` effect and
    /// dispatches the turn. There is no shell-side re-decision of origin;
    /// the `origin` field is forwarded into the DSL input verbatim.
    ///
    /// Boxed so the `ContentInput` + render/handling metadata doesn't widen
    /// the command channel's per-message stack footprint for every variant.
    SubmitWork {
        payload: Box<SubmitWorkPayload>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Sender-aware peer communication between mob members.
    ///
    /// This is not work-lane ingress. The actor resolves the sender member's
    /// comms runtime and the recipient peer route from the mob wiring graph,
    /// then submits a typed `CommsCommand::PeerMessage`.
    SendPeerMessage {
        from: AgentIdentity,
        to: AgentIdentity,
        content: ContentInput,
        handling_mode: meerkat_core::types::HandlingMode,
        reply_tx: oneshot::Sender<Result<meerkat_core::comms::SendReceipt, MobError>>,
    },
    /// Install (or clear) the host-owned outbound content-taint declaration
    /// on a member's comms runtime.
    ///
    /// Host-set carrier config, not machine state: the HOST owns the
    /// "this member's session content is tainted" fact; the member runtime
    /// stamps the declaration inside the signed region of every outbound
    /// content-bearing envelope until changed. Local members install on
    /// their session comms runtime; external members relay over the
    /// supervisor bridge.
    DeclareMemberOutboundTaint {
        identity: AgentIdentity,
        taint: Option<meerkat_core::comms::SenderContentTaint>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Unified work-lane cancellation: the MobMachine DSL owns live-runtime
    /// membership, fence-token freshness, and phase legality via the
    /// `CancelAllWork` transition. The actor feeds the machine, then
    /// interrupts the member when the transition lands.
    CancelAllWork {
        runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    #[cfg(feature = "runtime-adapter")]
    KickoffOutcomeResolved {
        agent_identity: AgentIdentity,
        outcome: Result<
            meerkat_runtime::completion::CompletionOutcome,
            meerkat_runtime::completion::CompletionWaitError,
        >,
        ack_tx: oneshot::Sender<()>,
    },
    RunFlow {
        flow_id: FlowId,
        activation_params: serde_json::Value,
        scoped_event_tx: Option<tokio::sync::mpsc::Sender<meerkat_core::ScopedAgentEvent>>,
        reply_tx: oneshot::Sender<Result<RunId, MobError>>,
    },
    PreviewRunFlowAdmission {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    CancelFlow {
        run_id: RunId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    FlowStatus {
        run_id: RunId,
        reply_tx: oneshot::Sender<Result<Option<MobRun>, MobError>>,
    },
    CommitFlowRunCommand {
        run_id: RunId,
        command: Box<MobMachineFlowRunCommand>,
        context: &'static str,
        reply_tx: oneshot::Sender<Result<Option<Vec<flow_run::Effect>>, MobError>>,
    },
    CommitFlowTerminalization {
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
        command: Box<MobMachineFlowRunCommand>,
        context: &'static str,
        reply_tx: oneshot::Sender<Result<TerminalizationOutcome, MobError>>,
    },
    CommitFlowFrameStorePlan {
        run_id: RunId,
        plan: Box<FlowFrameLoopStorePlan>,
        reply_tx: oneshot::Sender<Result<bool, MobError>>,
    },
    ProjectMachineInput {
        input: Box<mob_dsl::MobMachineInput>,
        reply_tx: oneshot::Sender<Result<mob_dsl::MobMachineState, MobError>>,
    },
    ApplyMachineInputEffects {
        input: Box<mob_dsl::MobMachineInput>,
        reply_tx: oneshot::Sender<Result<Vec<mob_dsl::MobMachineEffect>, MobError>>,
    },
    /// Actor-linearized no-op used by fenced member-operator execution before
    /// any operation whose implementation may otherwise be projection-only.
    /// The command gate performs the exact residency validation; the handler
    /// has no machine or shell effect.
    ValidateCommandAuthority {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Actor-linearized operator admission for composite/projection paths
    /// whose first semantic action is not itself a scoped MobCommand.
    AdmitControlScope {
        required: mob_dsl::ControlScope,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Actor-authorized capacity reclamation for the durable member-operator
    /// ledger. The handler snapshots exact current placed residencies from
    /// MobMachine and lets the store delete only rows outside that closed set.
    PruneStaleMemberOperatorRequests {
        reply_tx: oneshot::Sender<Result<u64, MobError>>,
    },
    /// Actor-serialized remote-turn sequence reservation. The caller supplies
    /// every exact obligation field except `dispatch_sequence`; the actor
    /// mints the next sequence and records custody in one authority turn.
    ReserveRemoteTurnObligation {
        intent: Box<crate::run::MobRunRemoteTurnIntent>,
        reply_tx: oneshot::Sender<
            Result<super::remote_flow_ticket::ReservedRemoteTurnIntent, MobError>,
        >,
    },
    /// Atomically persist a replay-complete run receipt, then commit the exact
    /// controller custody transition. Store-first ordering makes a crash
    /// between IO and in-memory commit recoverable from the receipt.
    CommitRemoteTurnReceipt {
        receipt: Box<crate::run::MobRunRemoteTurnReceipt>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Lifecycle recovery received an authenticated exact tracked-input
    /// cancellation receipt. Persist the canonical step terminal and close
    /// this one remote-turn obligation through actor-owned machine custody.
    CloseRemoteTurnAfterTrackedCancel {
        receipt: Box<crate::run::MobRunRemoteTurnReceipt>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    EnsureRemoteTurnRecord {
        obligation: Box<crate::event::RemoteTurnObligationEvent>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    FinalizeRemoteTurnPrivacyCleanup {
        cleanup: Box<super::remote_turn_reconciler::FinalizedRemoteTurnPrivacyCleanup>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ConvergeRecoveredFlowRun {
        run_id: RunId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ResolveRemoteTurnOutcome {
        obligation: Box<mob_dsl::RemoteTurnObligation>,
        record: super::bridge_protocol::BridgeTurnOutcomeRecord,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    AcknowledgeRemoteTurnOutcome {
        obligation: Box<mob_dsl::RemoteTurnObligation>,
        ack: super::bridge_protocol::BridgeTurnOutcomeAck,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    RequestPlacedCompletionCancellation {
        obligation: Box<crate::event::PlacedCompletionObligationEvent>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ResolvePlacedCompletionOutcome {
        obligation: Box<crate::event::PlacedCompletionObligationEvent>,
        record: super::bridge_protocol::BridgeTurnOutcomeRecord,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ClosePlacedCompletionOutcome {
        obligation: Box<crate::event::PlacedCompletionObligationEvent>,
        closure: crate::event::PlacedCompletionClosureEvent,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    AcknowledgePlacedCompletionOutcome {
        obligation: Box<crate::event::PlacedCompletionObligationEvent>,
        ack: super::bridge_protocol::BridgeTurnOutcomeAck,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ResolvePlacedKickoffOutcome {
        obligation: Box<crate::event::PlacedKickoffObligationEvent>,
        record: super::bridge_protocol::BridgeTurnOutcomeRecord,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ResolvePlacedKickoffCancelled {
        obligation: Box<crate::event::PlacedKickoffObligationEvent>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    AcknowledgePlacedKickoffOutcome {
        obligation: Box<crate::event::PlacedKickoffObligationEvent>,
        ack: super::bridge_protocol::BridgeTurnOutcomeAck,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    RejectPlacedKickoffBeforeAdmission {
        obligation: Box<crate::event::PlacedKickoffObligationEvent>,
        error: String,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    PreviewMachineInput {
        input: Box<mob_dsl::MobMachineInput>,
        reply_tx: oneshot::Sender<Result<mob_dsl::MobMachineState, MobError>>,
    },
    QueryMachineState {
        reply_tx: oneshot::Sender<mob_dsl::MobMachineState>,
    },
    #[cfg(test)]
    AuthorizeMemberTrustCleanupForTest {
        edge: mob_dsl::WiringEdge,
        reply_tx: oneshot::Sender<
            Result<
                crate::generated::protocol_mob_member_trust_unwiring::MobMemberTrustUnwiringObligation,
                MobError,
            >,
        >,
    },
    /// Deterministically stage a provisioned pending-spawn shell capability
    /// for an already committed member. The public Retire command then drives
    /// the real classifier, machine cancellation, abort, and anchor retention.
    #[cfg(test)]
    StagePendingSpawnForRetireTest {
        agent_identity: AgentIdentity,
        pending_spawn_session_id: SessionId,
        operation_id: meerkat_core::ops::OperationId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ApplyExternalPeerReciprocalTrust {
        key: mob_dsl::ExternalPeerKey,
        target_comms: std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>,
        peer: meerkat_core::comms::TrustedPeerDescriptor,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ProjectMachineSignal {
        signal: mob_dsl::MobMachineSignal,
    },
    /// Ready-wait observed a missing bridge-session snapshot for an active member.
    /// Routed through the actor's binding-currency-guarded
    /// `record_missing_member_bridge_session` (the same path `member_status` uses)
    /// so a concurrent rebind during the `read` cannot tip a still-current member
    /// into `RecoverMemberRestoreFailure`. Shell observation only — the actor owns
    /// whether to emit the restore-failure signal.
    RecordMissingMemberBridgeSession {
        agent_identity: crate::ids::AgentIdentity,
        bridge_session_id: SessionId,
    },
    FlowFinished {
        run_id: RunId,
    },
    FlowCanceledCleanup {
        run_id: RunId,
        terminalized: bool,
    },
    #[cfg(test)]
    FlowTrackerCounts {
        reply_tx: oneshot::Sender<(usize, usize)>,
    },
    #[cfg(test)]
    OrchestratorSnapshot {
        reply_tx: oneshot::Sender<MobOrchestratorSnapshot>,
    },
    #[cfg(test)]
    LifecycleSnapshot {
        reply_tx: oneshot::Sender<MobLifecycleSnapshot>,
    },
    #[cfg(test)]
    LifecycleNotificationBurst {
        count: usize,
        message: String,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Snapshot the T2 DSL field projections (member state markers, wiring
    /// edges, identity→runtime map, tasks + task id sets) directly from the
    /// DSL authority. Test-only read seam used by the runtime-parity
    /// snapshot so external shell code never has to keep a shadow copy
    /// (dogma #1, #13).
    #[cfg(test)]
    DslT2Snapshot {
        reply_tx: oneshot::Sender<MobDslT2Snapshot>,
    },
    StartupKickoffSnapshot {
        reply_tx: oneshot::Sender<MobStartupKickoffSnapshot>,
    },
    ProjectMemberList {
        include_retiring: bool,
        reply_tx: oneshot::Sender<Result<Vec<super::MobMemberListEntry>, crate::MobError>>,
    },
    ProjectMemberStatus {
        agent_identity: crate::ids::AgentIdentity,
        reply_tx: oneshot::Sender<Result<super::MobMemberSnapshot, crate::MobError>>,
    },
    ApplyIdentityDeclarationManifest {
        manifest: Box<crate::identity::IdentityDeclarationManifest>,
        reply_tx: oneshot::Sender<
            Result<crate::identity::IdentityDeclarationManifestApplyOutcome, crate::MobError>,
        >,
    },
    GetIdentityIntent {
        agent_identity: crate::ids::AgentIdentity,
        reply_tx: oneshot::Sender<
            Result<
                crate::identity::IdentityStoredObservation<crate::identity::IdentityIntentRecord>,
                crate::MobError,
            >,
        >,
    },
    GetIdentityDeclarationReceipt {
        scope_id: crate::identity::IdentityDeclarationScopeId,
        operation_id: meerkat_core::ops::OperationId,
        reply_tx: oneshot::Sender<
            Result<
                crate::identity::IdentityStoredObservation<
                    crate::identity::IdentityOperationReceipt,
                >,
                crate::MobError,
            >,
        >,
    },
    GetIdentityConvergenceStatus {
        agent_identity: crate::ids::AgentIdentity,
        reply_tx: oneshot::Sender<
            Result<
                crate::identity::IdentityStoredObservation<
                    crate::identity::IdentityConvergenceStatus,
                >,
                crate::MobError,
            >,
        >,
    },
    ConcludeObjective {
        agent_identity: crate::ids::AgentIdentity,
        objective_id: meerkat_core::interaction::ObjectiveId,
        outcome: String,
        reply_tx: oneshot::Sender<Result<(), crate::MobError>>,
    },
    BindObjectiveOwner {
        owner_identity: crate::ids::AgentIdentity,
        objective_id: meerkat_core::interaction::ObjectiveId,
        reply_tx: oneshot::Sender<Result<(), crate::MobError>>,
    },
    MemberMachineProjection {
        agent_identity: crate::ids::AgentIdentity,
        reply_tx: oneshot::Sender<Result<MobMemberMachineProjection, crate::MobError>>,
    },
    /// Read one member's transcript page through the placement-switched
    /// history proxy (§19.L2 / §7.4 phase 6). Local members serve the local
    /// session read; placed members proxy `ReadMemberHistory` over the
    /// supervisor bridge. `ReadHistory`-scoped at chokepoint (a).
    MemberHistory {
        agent_identity: crate::ids::AgentIdentity,
        from_index: Option<u64>,
        limit: Option<u32>,
        reply_tx: oneshot::Sender<
            Result<super::member_history_proxy::MemberHistoryPageDomain, crate::MobError>,
        >,
    },
    /// Hard-cancel a member's in-flight run (§7.4 phase 6). Distinct from
    /// [`Self::ForceCancel`] (cooperative boundary cancel) by design: this is
    /// bounded exact-run convergence over the immediate user-interrupt
    /// authority. A placed-member reply is successful only after the observed
    /// run is unbound/terminal, and retrying that operation cannot interrupt a
    /// newer run. `Cancel`-scoped at chokepoint (a); placed members are
    /// capability-gated on the recorded host `hard_cancel_member` fact BEFORE
    /// any bridge dispatch.
    HardCancelMember {
        agent_identity: AgentIdentity,
        reason: String,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Open a live channel on a member (§16 phase 6b, DEC-P6B-C2).
    /// Identity-addressed and placement-blind (DL3); `Live`-scoped at
    /// chokepoint (a). There is deliberately NO MobMachine input for the
    /// live family: controlling-side admission is scope gate + roster +
    /// placement/capability gate, and channel-binding admission is the
    /// OWNING session's MeerkatMachine authority (§16.3) — the MobMachine
    /// holds zero live-channel facts (DL2).
    MemberLiveOpen {
        agent_identity: AgentIdentity,
        turning_mode: Option<super::bridge_protocol::RealtimeTurningMode>,
        transport: Option<super::bridge_protocol::LiveOpenTransport>,
        reply_tx: oneshot::Sender<Result<MemberLiveOpenDelivery, MobError>>,
    },
    /// Close one NAMED live channel (DEC-P6B-C9: close-what-you-name is the
    /// CAS-like property that keeps a reconciling console from race-killing
    /// a concurrently re-minted channel). `Live`-scoped.
    MemberLiveClose {
        agent_identity: AgentIdentity,
        channel_id: String,
        reply_tx: oneshot::Sender<Result<super::bridge_protocol::LiveCloseStatus, MobError>>,
    },
    /// The dedicated live point read (§16.9 / DEC-P6B-C10): `channel_id:
    /// None` = "the member's active channel" (ADJ-P6B-2) — the reply-loss
    /// discovery primitive. `member_status` stays live-free and bridge-free;
    /// this is the ONLY remote channel-state path. `Live`-scoped.
    MemberLiveStatus {
        agent_identity: AgentIdentity,
        channel_id: Option<String>,
        reply_tx: oneshot::Sender<
            Result<super::member_live_proxy::MemberLiveStatusDomain, MobError>,
        >,
    },
    /// Drive one turn-level live control verb (DL10's closed vocabulary:
    /// commit_input / interrupt / truncate / refresh — frame-level input
    /// rides the direct WS, never the bridge). `Live`-scoped.
    MemberLiveControl {
        agent_identity: AgentIdentity,
        channel_id: String,
        verb: super::bridge_protocol::BridgeLiveControlVerb,
        reply_tx:
            oneshot::Sender<Result<super::bridge_protocol::BridgeLiveControlOutcome, MobError>>,
    },
    /// Internal pump-liveness poke (A17): ensure the member-event pump for a
    /// placed member is running because a remote-turn obligation is
    /// outstanding. Named cross-lane seam realization
    /// (`ensure_pump_for_obligation`); internal lane, no principal semantics.
    EnsureMemberEventPump {
        agent_identity: AgentIdentity,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Internal realization of a MACHINE-authorized external event
    /// subscription (the `AuthorizeExternalAgentEventSubscription` third
    /// outcome / `external_members` fan-out): ensure the pump and open an
    /// `AttributedEvent` tap. Authorization already happened at the machine
    /// input — this command is pure realization.
    EnsureMemberEventTap {
        agent_identity: AgentIdentity,
        reply_tx: oneshot::Sender<
            Result<tokio::sync::mpsc::Receiver<crate::event::AttributedEvent>, MobError>,
        >,
    },
    Stop {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ResumeLifecycle {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Complete {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Destroy {
        reply_tx: oneshot::Sender<
            Result<super::handle::MobDestroyReport, super::handle::MobDestroyError>,
        >,
    },
    Reset {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    RotateSupervisor {
        reply_tx: oneshot::Sender<Result<super::handle::SupervisorRotationReport, MobError>>,
    },
    /// §7.2 step 2: bind a member-host daemon to this mob's supervisor
    /// authority. Boxed: the request carries descriptor-derived identity and
    /// ceremony material that would widen every command variant.
    BindHost {
        request: Box<super::handle::HostBindRequest>,
        reply_tx: oneshot::Sender<Result<super::handle::HostBindReport, MobError>>,
    },
    /// Revoke a bound (or bind-requested) member host. Bound hosts must first
    /// complete the authenticated remote revoke terminal; the local machine
    /// and durable authority row clear only after the host's durable receipt
    /// is validated. Replies with the typed [`HostRevokeReport`] naming the
    /// member identities whose placements pointed at the revoked host
    /// (ADJ-P7-2 — never fabricated surface-side).
    ///
    /// [`HostRevokeReport`]: super::handle::HostRevokeReport
    RevokeHost {
        host_id: String,
        reply_tx: oneshot::Sender<Result<super::handle::HostRevokeReport, MobError>>,
    },
    /// Record (full-replace) a principal's control-scope grant (§8). The
    /// actor arm gates `caller` on `AdminGrants` via the sealed
    /// `ResolvedControlPolicy` BEFORE the machine input is built, then runs
    /// the prepare → durable-record → commit choreography under the
    /// `GrantRecorded` transition witness.
    GrantScopes {
        caller: crate::control_policy::MobControlPrincipal,
        principal: meerkat_core::auth::PrincipalId,
        scopes: std::collections::BTreeSet<mob_dsl::ControlScope>,
        expires_at_ms: Option<u64>,
        reply_tx: oneshot::Sender<Result<crate::control_policy::OperatorGrant, MobError>>,
    },
    /// Revoke scopes (`None` = the entire grant). The actor computes the
    /// `(revoked, remaining)` partition from recorded machine state (the
    /// machine revalidates it), gated on `AdminGrants` like its siblings.
    /// Replies whether the principal's grant record was removed entirely.
    RevokeScopes {
        caller: crate::control_policy::MobControlPrincipal,
        principal: meerkat_core::auth::PrincipalId,
        scopes: Option<std::collections::BTreeSet<mob_dsl::ControlScope>>,
        reply_tx: oneshot::Sender<Result<bool, MobError>>,
    },
    /// List raw grant records from machine state (expired rows included,
    /// verbatim `expires_at_ms`, no expired flag — one clock, one evaluator,
    /// at the enforcement seam only). `AdminGrants`-gated like its siblings.
    Grants {
        caller: crate::control_policy::MobControlPrincipal,
        reply_tx: oneshot::Sender<Result<Vec<crate::control_policy::OperatorGrant>, MobError>>,
    },
    PollEvents {
        after_cursor: u64,
        limit: usize,
        reply_tx: oneshot::Sender<Result<Vec<crate::event::MobEvent>, MobError>>,
    },
    ReplayAllEvents {
        reply_tx: oneshot::Sender<Result<Vec<crate::event::MobEvent>, MobError>>,
    },
    RecordOperatorActionProvenance {
        tool_name: String,
        authority_context: meerkat_core::service::MobToolAuthorityContext,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ForceCancel {
        agent_identity: AgentIdentity,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Wire a local mob member to a peer target.
    ///
    /// D-wire-handler (#26): the MobMachine DSL owns wiring-graph authority.
    /// Local member targets route through `WireMembers { edge }`; raw
    /// external peer descriptors route through `WireExternalPeer { key, edge }`
    /// so local/name uniqueness and descriptor/key/address truth are not
    /// collapsed into a member `WiringEdge`.
    Wire {
        local: AgentIdentity,
        target: super::handle::PeerTarget,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Materialize many local-member wiring edges through one actor turn.
    WireMembersBatch {
        edges: Vec<(AgentIdentity, AgentIdentity)>,
        reply_tx: oneshot::Sender<Result<super::handle::MobWireMembersBatchReport, MobError>>,
    },
    /// Unwire a local mob member from a peer target. Mirror of `Wire`:
    /// local member targets forward to `UnwireMembers`, while raw external
    /// descriptors forward to `UnwireExternalPeer`.
    Unwire {
        local: AgentIdentity,
        target: super::handle::PeerTarget,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Drain outstanding cross-host route-install obligations (multi-host
    /// §10.4, ADJ-P4-9b): the ONE canonical drain verb — DRAIN-ONLY
    /// (ADJ-P4-15). Re-derivation from durable `wiring_edges` ×
    /// `member_placement` belongs to the rebind/recovery/revival triggers,
    /// never to this verb: an idle drive sends nothing (T-W3 depends on
    /// that idempotency). Realizes every already-pending Install as an
    /// `InstallPeerTrust` bridge send. Per-install failures leave the row
    /// pending (fail closed, observable via the route-installs projection)
    /// and never fail the drain; a non-Install row is an invariant error.
    DriveRouteInstalls {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    SetSpawnPolicy {
        policy: Option<Arc<dyn super::spawn_policy::SpawnPolicy>>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Shutdown {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Test-support-only process-death simulation. Unlike `Shutdown`, this
    /// stops volatile actor work without committing lifecycle/run terminals.
    #[cfg(any(test, feature = "test-support"))]
    CrashStopPreservingDurableWorkForTest {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Read the current lifecycle phase directly from the DSL authority.
    /// Routes through the command channel so the actor returns the single
    /// canonical DSL-authority value; there is no atomic shadow (dogma #1,
    /// #13, #17).
    QueryPhase {
        reply_tx: oneshot::Sender<Result<MobState, MobError>>,
    },
}

impl MobCommand {
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Spawn { .. } => "Spawn",
            Self::SpawnProvisioned { .. } => "SpawnProvisioned",
            Self::RevivePlacedMember { .. } => "RevivePlacedMember",
            Self::HostStatusPollCompleted { .. } => "HostStatusPollCompleted",
            Self::HostRuntimeIncarnationObserved { .. } => "HostRuntimeIncarnationObserved",
            Self::HostOrphanReleaseCompleted { .. } => "HostOrphanReleaseCompleted",
            Self::PlacedBehaviorCompleted { .. } => "PlacedBehaviorCompleted",
            Self::Retire { .. } => "Retire",
            Self::Respawn { .. } => "Respawn",
            Self::RetireAll { .. } => "RetireAll",
            Self::SubmitWork { .. } => "SubmitWork",
            Self::SendPeerMessage { .. } => "SendPeerMessage",
            Self::DriveRouteInstalls { .. } => "DriveRouteInstalls",
            Self::DeclareMemberOutboundTaint { .. } => "DeclareMemberOutboundTaint",
            Self::CancelAllWork { .. } => "CancelAllWork",
            #[cfg(feature = "runtime-adapter")]
            Self::KickoffOutcomeResolved { .. } => "KickoffOutcomeResolved",
            Self::RunFlow { .. } => "RunFlow",
            Self::PreviewRunFlowAdmission { .. } => "PreviewRunFlowAdmission",
            Self::CancelFlow { .. } => "CancelFlow",
            Self::FlowStatus { .. } => "FlowStatus",
            Self::CommitFlowRunCommand { .. } => "CommitFlowRunCommand",
            Self::CommitFlowTerminalization { .. } => "CommitFlowTerminalization",
            Self::CommitFlowFrameStorePlan { .. } => "CommitFlowFrameStorePlan",
            Self::ProjectMachineInput { .. } => "ProjectMachineInput",
            Self::ApplyMachineInputEffects { .. } => "ApplyMachineInputEffects",
            Self::ValidateCommandAuthority { .. } => "ValidateCommandAuthority",
            Self::AdmitControlScope { .. } => "AdmitControlScope",
            Self::PruneStaleMemberOperatorRequests { .. } => "PruneStaleMemberOperatorRequests",
            Self::ReserveRemoteTurnObligation { .. } => "ReserveRemoteTurnObligation",
            Self::CommitRemoteTurnReceipt { .. } => "CommitRemoteTurnReceipt",
            Self::CloseRemoteTurnAfterTrackedCancel { .. } => "CloseRemoteTurnAfterTrackedCancel",
            Self::EnsureRemoteTurnRecord { .. } => "EnsureRemoteTurnRecord",
            Self::FinalizeRemoteTurnPrivacyCleanup { .. } => "FinalizeRemoteTurnPrivacyCleanup",
            Self::ConvergeRecoveredFlowRun { .. } => "ConvergeRecoveredFlowRun",
            Self::ResolveRemoteTurnOutcome { .. } => "ResolveRemoteTurnOutcome",
            Self::AcknowledgeRemoteTurnOutcome { .. } => "AcknowledgeRemoteTurnOutcome",
            Self::RequestPlacedCompletionCancellation { .. } => {
                "RequestPlacedCompletionCancellation"
            }
            Self::ResolvePlacedCompletionOutcome { .. } => "ResolvePlacedCompletionOutcome",
            Self::ClosePlacedCompletionOutcome { .. } => "ClosePlacedCompletionOutcome",
            Self::AcknowledgePlacedCompletionOutcome { .. } => "AcknowledgePlacedCompletionOutcome",
            Self::ResolvePlacedKickoffOutcome { .. } => "ResolvePlacedKickoffOutcome",
            Self::ResolvePlacedKickoffCancelled { .. } => "ResolvePlacedKickoffCancelled",
            Self::AcknowledgePlacedKickoffOutcome { .. } => "AcknowledgePlacedKickoffOutcome",
            Self::RejectPlacedKickoffBeforeAdmission { .. } => "RejectPlacedKickoffBeforeAdmission",
            Self::PreviewMachineInput { .. } => "PreviewMachineInput",
            Self::QueryMachineState { .. } => "QueryMachineState",
            #[cfg(test)]
            Self::AuthorizeMemberTrustCleanupForTest { .. } => "AuthorizeMemberTrustCleanupForTest",
            #[cfg(test)]
            Self::StagePendingSpawnForRetireTest { .. } => "StagePendingSpawnForRetireTest",
            Self::ApplyExternalPeerReciprocalTrust { .. } => "ApplyExternalPeerReciprocalTrust",
            Self::ProjectMachineSignal { .. } => "ProjectMachineSignal",
            Self::RecordMissingMemberBridgeSession { .. } => "RecordMissingMemberBridgeSession",
            Self::FlowFinished { .. } => "FlowFinished",
            Self::FlowCanceledCleanup { .. } => "FlowCanceledCleanup",
            #[cfg(test)]
            Self::FlowTrackerCounts { .. } => "FlowTrackerCounts",
            #[cfg(test)]
            Self::OrchestratorSnapshot { .. } => "OrchestratorSnapshot",
            #[cfg(test)]
            Self::LifecycleSnapshot { .. } => "LifecycleSnapshot",
            #[cfg(test)]
            Self::LifecycleNotificationBurst { .. } => "LifecycleNotificationBurst",
            #[cfg(test)]
            Self::DslT2Snapshot { .. } => "DslT2Snapshot",
            Self::StartupKickoffSnapshot { .. } => "StartupKickoffSnapshot",
            Self::ProjectMemberList { .. } => "ProjectMemberList",
            Self::ProjectMemberStatus { .. } => "ProjectMemberStatus",
            Self::ApplyIdentityDeclarationManifest { .. } => "ApplyIdentityDeclarationManifest",
            Self::GetIdentityIntent { .. } => "GetIdentityIntent",
            Self::GetIdentityDeclarationReceipt { .. } => "GetIdentityDeclarationReceipt",
            Self::GetIdentityConvergenceStatus { .. } => "GetIdentityConvergenceStatus",
            Self::ConcludeObjective { .. } => "ConcludeObjective",
            Self::BindObjectiveOwner { .. } => "BindObjectiveOwner",
            Self::MemberMachineProjection { .. } => "MemberMachineProjection",
            Self::Stop { .. } => "Stop",
            Self::ResumeLifecycle { .. } => "ResumeLifecycle",
            Self::Complete { .. } => "Complete",
            Self::Destroy { .. } => "Destroy",
            Self::Reset { .. } => "Reset",
            Self::RotateSupervisor { .. } => "RotateSupervisor",
            Self::BindHost { .. } => "BindHost",
            Self::RevokeHost { .. } => "RevokeHost",
            Self::GrantScopes { .. } => "GrantScopes",
            Self::RevokeScopes { .. } => "RevokeScopes",
            Self::Grants { .. } => "Grants",
            Self::PollEvents { .. } => "PollEvents",
            Self::ReplayAllEvents { .. } => "ReplayAllEvents",
            Self::RecordOperatorActionProvenance { .. } => "RecordOperatorActionProvenance",
            Self::ForceCancel { .. } => "ForceCancel",
            Self::HardCancelMember { .. } => "HardCancelMember",
            Self::MemberHistory { .. } => "MemberHistory",
            Self::MemberLiveOpen { .. } => "MemberLiveOpen",
            Self::MemberLiveClose { .. } => "MemberLiveClose",
            Self::MemberLiveStatus { .. } => "MemberLiveStatus",
            Self::MemberLiveControl { .. } => "MemberLiveControl",
            Self::EnsureMemberEventPump { .. } => "EnsureMemberEventPump",
            Self::EnsureMemberEventTap { .. } => "EnsureMemberEventTap",
            Self::Wire { .. } => "Wire",
            Self::WireMembersBatch { .. } => "WireMembersBatch",
            Self::Unwire { .. } => "Unwire",
            Self::SetSpawnPolicy { .. } => "SetSpawnPolicy",
            Self::Shutdown { .. } => "Shutdown",
            #[cfg(any(test, feature = "test-support"))]
            Self::CrashStopPreservingDurableWorkForTest { .. } => {
                "CrashStopPreservingDurableWorkForTest"
            }
            Self::QueryPhase { .. } => "QueryPhase",
        }
    }
}
