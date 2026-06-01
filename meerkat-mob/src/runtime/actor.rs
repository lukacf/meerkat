use super::disposal::{
    AbortOnError, BulkBestEffort, DisposalContext, DisposalReport, DisposalStep, ErrorPolicy,
    WarnAndContinue,
};
use super::flow_frame_engine::FlowFrameLoopStorePlan;
use super::mob_member_lifecycle_projection::{
    CanonicalMemberSnapshotMaterial, MobMemberLifecycleInput, MobMemberLifecycleProjection,
    kickoff_snapshot_from_machine_state,
};
use super::mob_runtime_bridge_authority::{MobRuntimeBridgeAuthority, MobRuntimeBridgeEffect};
use super::provision_guard::PendingProvision;
use super::terminalization::{TerminalizationOutcome, TerminalizationTarget};
use super::transaction::LifecycleRollback;
use super::*;
use crate::generated::protocol_mob_destroying_session_ingress::MobDestroyingSessionIngressObligation;
use crate::ids::{AgentIdentity, AgentRuntimeId, RespawnTopologyPeerId};
use crate::machines::mob_machine as mob_dsl;
use crate::run::{MobMachineFlowAuthorityToken, MobMachineFlowRunCommand, MobRunStatus, flow_run};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use futures::FutureExt;
use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::comms::{
    CommsTrustMutation, CommsTrustMutationAuthority, CommsTrustMutationResult, PeerAddress,
    PeerLifecycleKind, PeerName, PeerRoute, SendError, TrustedPeerDescriptor,
};
use meerkat_core::time_compat::SystemTime;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

/// Lightweight handle for a spawned autonomous initial turn.
///
/// The `JoinHandle` is for abort on stop/dispose.
pub(super) struct InitialTurnHandle {
    handle: tokio::task::JoinHandle<()>,
}

impl InitialTurnHandle {
    fn abort(self) {
        self.handle.abort();
    }
}

enum SubmitWorkDispatchCompletion {
    Completed,
    AwaitTurnAdmission {
        operation_id: Option<meerkat_core::ops::OperationId>,
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
    },
    AwaitTurnCompletion {
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
    },
}

impl SubmitWorkDispatchCompletion {
    fn kind(&self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::AwaitTurnAdmission { .. } => "AwaitTurnAdmission",
            Self::AwaitTurnCompletion { .. } => "AwaitTurnCompletion",
        }
    }
}

struct SubmitWorkDispatchRequest {
    content: ContentInput,
    handling_mode: meerkat_core::types::HandlingMode,
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
    ack_mode: crate::mob_machine::SubmitWorkAckMode,
    operation_id: Option<meerkat_core::ops::OperationId>,
}

#[derive(Debug, Clone)]
enum SubmitWorkIngressAuthority {
    Runtime {
        agent_runtime_id: mob_dsl::AgentRuntimeId,
        fence_token: mob_dsl::FenceToken,
        generation: Option<mob_dsl::Generation>,
        session_id: mob_dsl::SessionId,
        work_id: mob_dsl::WorkId,
        origin: mob_dsl::WorkOrigin,
    },
    PeerRuntime {
        agent_runtime_id: mob_dsl::AgentRuntimeId,
        fence_token: mob_dsl::FenceToken,
        generation: Option<mob_dsl::Generation>,
        work_id: mob_dsl::WorkId,
        origin: mob_dsl::WorkOrigin,
    },
}

impl SubmitWorkIngressAuthority {
    fn from_transition(
        transition: &mob_dsl::MobMachineTransition,
        expected_runtime_id: &mob_dsl::AgentRuntimeId,
        expected_fence_token: mob_dsl::FenceToken,
        expected_generation: mob_dsl::Generation,
        expected_work_id: &mob_dsl::WorkId,
        expected_origin: mob_dsl::WorkOrigin,
    ) -> Result<Self, MobError> {
        let mut resolved = None;
        for effect in transition.effects() {
            let authority = match effect {
                mob_dsl::MobMachineEffect::RequestRuntimeIngress {
                    agent_runtime_id,
                    fence_token,
                    generation,
                    session_id,
                    work_id,
                    origin,
                } => Self::Runtime {
                    agent_runtime_id: agent_runtime_id.clone(),
                    fence_token: *fence_token,
                    generation: *generation,
                    session_id: session_id.clone(),
                    work_id: work_id.clone(),
                    origin: *origin,
                },
                mob_dsl::MobMachineEffect::RequestPeerRuntimeIngress {
                    agent_runtime_id,
                    fence_token,
                    generation,
                    work_id,
                    origin,
                } => Self::PeerRuntime {
                    agent_runtime_id: agent_runtime_id.clone(),
                    fence_token: *fence_token,
                    generation: *generation,
                    work_id: work_id.clone(),
                    origin: *origin,
                },
                _ => continue,
            };
            authority.verify_payload(
                expected_runtime_id,
                expected_fence_token,
                expected_generation,
                expected_work_id,
                expected_origin,
            )?;
            if resolved.replace(authority).is_some() {
                return Err(MobError::Internal(
                    "MobMachine SubmitWork emitted multiple generated runtime ingress authorities"
                        .to_string(),
                ));
            }
        }
        resolved.ok_or_else(|| {
            MobError::Internal(
                "MobMachine accepted SubmitWork but emitted no generated runtime ingress authority"
                    .to_string(),
            )
        })
    }

    fn variant(&self) -> &'static str {
        match self {
            Self::Runtime { .. } => "RequestRuntimeIngress",
            Self::PeerRuntime { .. } => "RequestPeerRuntimeIngress",
        }
    }

    fn agent_runtime_id(&self) -> &mob_dsl::AgentRuntimeId {
        match self {
            Self::Runtime {
                agent_runtime_id, ..
            }
            | Self::PeerRuntime {
                agent_runtime_id, ..
            } => agent_runtime_id,
        }
    }

    fn fence_token(&self) -> mob_dsl::FenceToken {
        match self {
            Self::Runtime { fence_token, .. } | Self::PeerRuntime { fence_token, .. } => {
                *fence_token
            }
        }
    }

    fn generation(&self) -> Option<mob_dsl::Generation> {
        match self {
            Self::Runtime { generation, .. } | Self::PeerRuntime { generation, .. } => *generation,
        }
    }

    fn work_id(&self) -> &mob_dsl::WorkId {
        match self {
            Self::Runtime { work_id, .. } | Self::PeerRuntime { work_id, .. } => work_id,
        }
    }

    fn origin(&self) -> mob_dsl::WorkOrigin {
        match self {
            Self::Runtime { origin, .. } | Self::PeerRuntime { origin, .. } => *origin,
        }
    }

    fn verify_payload(
        &self,
        expected_runtime_id: &mob_dsl::AgentRuntimeId,
        expected_fence_token: mob_dsl::FenceToken,
        expected_generation: mob_dsl::Generation,
        expected_work_id: &mob_dsl::WorkId,
        expected_origin: mob_dsl::WorkOrigin,
    ) -> Result<(), MobError> {
        if self.agent_runtime_id() != expected_runtime_id
            || self.fence_token() != expected_fence_token
            || self.generation() != Some(expected_generation)
            || self.work_id() != expected_work_id
            || self.origin() != expected_origin
        {
            return Err(MobError::Internal(format!(
                "generated {} authority did not match admitted SubmitWork payload",
                self.variant()
            )));
        }
        Ok(())
    }

    fn verify_member_ref(&self, member_ref: &MemberRef, context: &str) -> Result<(), MobError> {
        match self {
            Self::Runtime { session_id, .. } => {
                let Some(bridge_session_id) = member_ref.bridge_session_id() else {
                    return Err(MobError::Internal(format!(
                        "{context} requires a session-bound member for generated {} authority",
                        self.variant()
                    )));
                };
                let projected_session_id = mob_dsl::SessionId::from_domain(bridge_session_id);
                if &projected_session_id != session_id {
                    return Err(MobError::Internal(format!(
                        "{context} generated {} authority session does not match MobMachine session binding",
                        self.variant()
                    )));
                }
                Ok(())
            }
            Self::PeerRuntime { .. } => match member_ref {
                MemberRef::BackendPeer {
                    session_id: None, ..
                } => Ok(()),
                _ => Err(MobError::Internal(format!(
                    "{context} requires a peer-only member for generated {} authority",
                    self.variant()
                ))),
            },
        }
    }

    fn is_peer_runtime(&self) -> bool {
        matches!(self, Self::PeerRuntime { .. })
    }
}

// Sized for real mob-scale startup/shutdown fan-out (50+ members).
#[cfg(not(target_arch = "wasm32"))]
const MAX_PARALLEL_REMOTE_MEMBER_TEARDOWNS: usize = 64;
const MAX_LIFECYCLE_NOTIFICATION_TASKS: usize = 16;
const MAX_PARALLEL_PEER_RETIRE_NOTIFICATIONS: usize = 64;
#[cfg(not(test))]
pub(super) const MAX_PENDING_PEER_DELIVERIES: usize = 1024;
#[cfg(test)]
pub(super) const MAX_PENDING_PEER_DELIVERIES: usize = 4;
#[cfg(test)]
pub(super) static SPAWN_PROVISIONED_COMMAND_DELAY_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn observed_runtime_id(signal: &mob_dsl::MobMachineSignal) -> Option<&mob_dsl::AgentRuntimeId> {
    match signal {
        mob_dsl::MobMachineSignal::ObserveRuntimeReady {
            agent_runtime_id, ..
        }
        | mob_dsl::MobMachineSignal::ObserveRuntimeRetired {
            agent_runtime_id, ..
        }
        | mob_dsl::MobMachineSignal::ObserveRuntimeDestroyed {
            agent_runtime_id, ..
        } => Some(agent_runtime_id),
        _ => None,
    }
}

fn foreign_runtime_observation<'a>(
    state: &mob_dsl::MobMachineState,
    signal: &'a mob_dsl::MobMachineSignal,
) -> Option<&'a mob_dsl::AgentRuntimeId> {
    let agent_runtime_id = observed_runtime_id(signal)?;
    (!state.live_runtime_ids.contains(agent_runtime_id)).then_some(agent_runtime_id)
}

#[derive(Clone)]
enum WiringEndpoint {
    Local {
        entry: Box<RosterEntry>,
        comms: Arc<dyn CoreCommsRuntime>,
        spec: TrustedPeerDescriptor,
        comms_name: String,
    },
    PeerOnly {
        spec: TrustedPeerDescriptor,
        binding: crate::RuntimeBinding,
    },
}

#[derive(Clone)]
struct LocalBatchWiringEndpoint {
    comms: Arc<dyn CoreCommsRuntime>,
    spec: TrustedPeerDescriptor,
    removal_key: String,
}

struct PeerMessageDeliveryPlan {
    from: MeerkatId,
    to: MeerkatId,
    sender_comms: Arc<dyn CoreCommsRuntime>,
    command: CommsCommand,
}

type PeerDeliveryId = u64;

pub(super) struct PeerDeliveryCompletion {
    id: PeerDeliveryId,
}

pub(super) struct PeerDeliveryInflight {
    from: MeerkatId,
    to: MeerkatId,
    cancel_token: tokio_util::sync::CancellationToken,
}

struct SupervisorPrivateTrustInstall {
    peer_id: String,
    epoch: u64,
    removal_key: String,
}

struct SupervisorAuthorityActivationError {
    error: MobError,
    rollback_succeeded: bool,
    rollback_error: Option<String>,
}

struct SupervisorAuthorityLoad {
    durable: crate::store::SupervisorAuthorityRecord,
}

struct SupervisorPendingRotationPersistence {
    pending_authority_recorded: bool,
    persisted_record: Option<crate::store::SupervisorAuthorityRecord>,
}

struct SupervisorUnrecordedAttemptRecovery {
    rollback_succeeded: bool,
    rollback_error: Option<String>,
}

struct PreparedSupervisorAuthorityPersistence {
    transition: PreparedDslTransition,
    authority: crate::store::SupervisorAuthorityPersistenceAuthority,
}

struct PreparedSupervisorAuthorityDeletion {
    transition: PreparedDslTransition,
    authority: crate::store::SupervisorAuthorityDeletionAuthority,
}

struct PreparedDslInput {
    authority: mob_dsl::MobMachinePreparedAuthority,
    effects: Vec<mob_dsl::MobMachineEffect>,
    phase_changed: bool,
}

struct PreparedDslTransition {
    authority: mob_dsl::MobMachinePreparedAuthority,
    transition: mob_dsl::MobMachineTransition,
}

struct BatchWireTrustApplication {
    edge: mob_dsl::WiringEdge,
    identity: AgentIdentity,
    peer_id: String,
    comms: Arc<dyn CoreCommsRuntime>,
    peer: TrustedPeerDescriptor,
    authority: CommsTrustMutationAuthority,
}

struct BatchWireTrustRollback {
    edge: mob_dsl::WiringEdge,
    identity: AgentIdentity,
    peer_id: String,
    comms: Arc<dyn CoreCommsRuntime>,
}

struct RetireTrustCleanupPlan {
    retiring_comms: Option<Arc<dyn CoreCommsRuntime>>,
    retiring_spec: Option<TrustedPeerDescriptor>,
    machine_wired_peer_identities: BTreeSet<AgentIdentity>,
    trust_unwire_authority_by_peer: BTreeMap<AgentIdentity, CommsTrustMutationAuthority>,
}

impl RetireTrustCleanupPlan {
    fn empty() -> Self {
        Self {
            retiring_comms: None,
            retiring_spec: None,
            machine_wired_peer_identities: BTreeSet::new(),
            trust_unwire_authority_by_peer: BTreeMap::new(),
        }
    }

    fn has_peers(&self) -> bool {
        !self.machine_wired_peer_identities.is_empty()
    }
}

#[derive(Debug, Clone)]
enum WireTrustAuthority {
    GraphAdded(MemberTrustHandoff),
    RepairRequested(MemberTrustHandoff),
    ExternalGraphAdded(CommsTrustMutationAuthority),
    ExternalRepairRequested(CommsTrustMutationAuthority),
}

impl WireTrustAuthority {
    fn dsl_added(&self) -> bool {
        matches!(self, Self::GraphAdded(_) | Self::ExternalGraphAdded(_))
    }

    fn is_repair(&self) -> bool {
        matches!(
            self,
            Self::RepairRequested(_) | Self::ExternalRepairRequested(_)
        )
    }

    fn member_handoff(&self) -> Result<&MemberTrustHandoff, MobError> {
        match self {
            Self::GraphAdded(handoff) | Self::RepairRequested(handoff) => Ok(handoff),
            Self::ExternalGraphAdded(_) | Self::ExternalRepairRequested(_) => {
                Err(MobError::WiringError(
                    "external peer authority does not carry member peer handoff".to_string(),
                ))
            }
        }
    }

    fn external_authority(&self) -> Result<&CommsTrustMutationAuthority, MobError> {
        match self {
            Self::ExternalGraphAdded(authority) | Self::ExternalRepairRequested(authority) => {
                Ok(authority)
            }
            Self::GraphAdded(_) | Self::RepairRequested(_) => Err(MobError::WiringError(
                "member trust authority does not carry external peer authority".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
struct MemberTrustHandoff {
    edge: mob_dsl::WiringEdge,
    authority: MemberTrustAuthority,
    operation: MemberTrustOperation,
}

#[derive(Debug, Clone)]
enum MemberTrustAuthority {
    Wiring(crate::generated::protocol_mob_member_trust_wiring::MobMemberTrustWiringObligation),
    Unwiring(
        crate::generated::protocol_mob_member_trust_unwiring::MobMemberTrustUnwiringObligation,
    ),
    Repair(crate::generated::protocol_mob_member_trust_wiring::MobMemberTrustWiringObligation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberTrustOperation {
    Wiring,
    Unwiring,
    Repair,
}

impl MemberTrustHandoff {
    fn peer_id_for(&self, identity: &AgentIdentity) -> Result<&str, MobError> {
        let identity = mob_dsl::AgentIdentity::from_domain(identity);
        let actual = match &self.authority {
            MemberTrustAuthority::Wiring(obligation) | MemberTrustAuthority::Repair(obligation) => {
                if obligation.edge().a == identity {
                    Some(obligation.a_peer_id().0.as_str())
                } else if obligation.edge().b == identity {
                    Some(obligation.b_peer_id().0.as_str())
                } else {
                    None
                }
            }
            MemberTrustAuthority::Unwiring(obligation) => {
                if obligation.edge().a == identity {
                    Some(obligation.a_peer_id().0.as_str())
                } else if obligation.edge().b == identity {
                    Some(obligation.b_peer_id().0.as_str())
                } else {
                    None
                }
            }
        };
        actual.ok_or_else(|| {
            MobError::WiringError(format!(
                "generated member trust obligation does not cover '{identity:?}'"
            ))
        })
    }

    fn require_peer_id_for(
        &self,
        identity: &AgentIdentity,
        expected_peer_id: &str,
    ) -> Result<(), MobError> {
        let actual = self.peer_id_for(identity)?;
        if actual == expected_peer_id {
            Ok(())
        } else {
            Err(MobError::WiringError(format!(
                "generated member trust handoff peer id '{actual}' does not match resolved peer id '{expected_peer_id}' for '{identity}'"
            )))
        }
    }

    fn wiring_authority_for(
        &self,
        identity: &AgentIdentity,
        expected_peer_id: &str,
        live_authority: &mob_dsl::MobMachineAuthority,
    ) -> Result<CommsTrustMutationAuthority, MobError> {
        match &self.authority {
            MemberTrustAuthority::Wiring(obligation) => {
                crate::generated::protocol_mob_member_trust_wiring::wiring_authority_for_identity_with_live_authority(
                    obligation,
                    identity.as_str(),
                    expected_peer_id,
                    live_authority,
                )
                .map_err(MobError::WiringError)
            }
            MemberTrustAuthority::Repair(_) | MemberTrustAuthority::Unwiring(_) => {
                Err(MobError::WiringError(
                    "generated member trust obligation cannot wire trust".to_string(),
                ))
            }
        }
    }

    fn unwiring_authority_for(
        &self,
        identity: &AgentIdentity,
        expected_peer_id: &str,
    ) -> Result<CommsTrustMutationAuthority, MobError> {
        match &self.authority {
            MemberTrustAuthority::Unwiring(obligation) => {
                crate::generated::protocol_mob_member_trust_unwiring::unwiring_authority_for_identity(
                    obligation,
                    identity.as_str(),
                    expected_peer_id,
                )
                .map_err(MobError::WiringError)
            }
            MemberTrustAuthority::Wiring(_) | MemberTrustAuthority::Repair(_) => {
                Err(MobError::WiringError(
                    "generated member trust obligation cannot unwire trust".to_string(),
                ))
            }
        }
    }

    fn repair_authority_for(
        &self,
        identity: &AgentIdentity,
        expected_peer_id: &str,
        live_authority: &mob_dsl::MobMachineAuthority,
    ) -> Result<CommsTrustMutationAuthority, MobError> {
        match &self.authority {
            MemberTrustAuthority::Repair(obligation) => {
                crate::generated::protocol_mob_member_trust_wiring::repair_authority_for_identity_with_live_authority(
                    obligation,
                    identity.as_str(),
                    expected_peer_id,
                    live_authority,
                )
                .map_err(MobError::WiringError)
            }
            MemberTrustAuthority::Wiring(_) | MemberTrustAuthority::Unwiring(_) => {
                Err(MobError::WiringError(
                    "generated member trust obligation cannot repair trust".to_string(),
                ))
            }
        }
    }

    fn add_authority_for(
        &self,
        identity: &AgentIdentity,
        expected_peer_id: &str,
        live_authority: &mob_dsl::MobMachineAuthority,
    ) -> Result<CommsTrustMutationAuthority, MobError> {
        match self.operation {
            MemberTrustOperation::Wiring => {
                self.wiring_authority_for(identity, expected_peer_id, live_authority)
            }
            MemberTrustOperation::Repair => {
                self.repair_authority_for(identity, expected_peer_id, live_authority)
            }
            MemberTrustOperation::Unwiring => Err(MobError::WiringError(
                "generated member unwiring handoff cannot add trust".to_string(),
            )),
        }
    }
}

/// Resolve the runtime binding for a spawn request.
///
/// `RuntimeBinding` takes precedence over the legacy `backend` tag. When neither
/// is provided, resolves from profile/definition defaults. `External` without
/// a concrete `RuntimeBinding` is an error — you cannot spawn an external
/// member without declaring the real process identity.
fn resolve_binding(
    binding: Option<crate::RuntimeBinding>,
    backend: Option<crate::MobBackendKind>,
    profile_backend: Option<crate::MobBackendKind>,
    definition_default: crate::MobBackendKind,
    agent_identity: &MeerkatId,
) -> Result<crate::RuntimeBinding, MobError> {
    if let Some(b) = binding {
        return Ok(b);
    }
    let kind = backend.or(profile_backend).unwrap_or(definition_default);
    match kind {
        crate::MobBackendKind::Session => Ok(crate::RuntimeBinding::Session),
        crate::MobBackendKind::External => Err(MobError::WiringError(format!(
            "external backend requires explicit RuntimeBinding for '{agent_identity}'"
        ))),
    }
}

fn normalize_runtime_mode_for_binding(
    runtime_mode: crate::MobRuntimeMode,
    binding: &crate::RuntimeBinding,
) -> crate::MobRuntimeMode {
    match binding {
        crate::RuntimeBinding::External { .. } => crate::MobRuntimeMode::TurnDriven,
        crate::RuntimeBinding::Session => runtime_mode,
    }
}

pub(super) fn admit_bridge_session_for_spawn(
    req: &mut meerkat_core::service::CreateSessionRequest,
) -> SessionId {
    if req.build.is_none() {
        req.build = Some(meerkat_core::service::SessionBuildOptions::default());
    }
    let build = req
        .build
        .as_mut()
        .expect("build options were initialized above");
    if let Some(session) = build.resume_session.as_ref() {
        return session.id().clone();
    }
    let session_id = SessionId::new();
    build.resume_session = Some(meerkat_core::session::Session::with_id(session_id.clone()));
    session_id
}

/// Project a DSL `MobPhase` into the shell `MobState` enum. Used by
/// `MobActor::state()` and by the `MobCommand::QueryPhase` reply so that
/// external `MobHandle::status()` callers observe the same DSL-authority
/// value the actor uses internally (dogma #1, #13, #17).
fn project_dsl_phase(phase: mob_dsl::MobPhase) -> MobState {
    match phase {
        mob_dsl::MobPhase::Running => MobState::Running,
        mob_dsl::MobPhase::Stopped => MobState::Stopped,
        mob_dsl::MobPhase::Completed => MobState::Completed,
        mob_dsl::MobPhase::Destroyed => MobState::Destroyed,
    }
}

/// Extract the pure wire runtime-state observation for MobMachine terminality
/// classification. This is a faithful 1:1 projection of the observed
/// `BridgeMemberRuntimeState`; the terminal/non-terminal verdict is decided by
/// MobMachine, not here. `BridgeMemberRuntimeState` is `#[non_exhaustive]`, so
/// an unrecognized wire state fails closed rather than being silently coerced
/// to a (non-terminal) default.
fn remote_member_runtime_observed_state(
    state: super::bridge_protocol::BridgeMemberRuntimeState,
) -> Result<mob_dsl::MobRemoteMemberRuntimeObservedState, MobError> {
    use super::bridge_protocol::BridgeMemberRuntimeState as Wire;
    use mob_dsl::MobRemoteMemberRuntimeObservedState as Observed;
    Ok(match state {
        Wire::Initializing => Observed::Initializing,
        Wire::Idle => Observed::Idle,
        Wire::Attached => Observed::Attached,
        Wire::Running => Observed::Running,
        Wire::Retired => Observed::Retired,
        Wire::Stopped => Observed::Stopped,
        Wire::Destroyed => Observed::Destroyed,
        other => {
            return Err(MobError::Internal(format!(
                "unrecognized remote-member runtime wire state `{other}` cannot be classified for terminality"
            )));
        }
    })
}

/// Map a typed wire bridge rejection cause onto the MobMachine observation
/// enum. The wire cause is `#[non_exhaustive]`; any future variant the mob does
/// not yet understand maps to `Internal`, which MobMachine classifies as
/// `FatalBubbleUp` — failing closed (no recovery) on an unrecognized cause.
fn mob_bridge_rejection_cause(
    cause: super::bridge_protocol::BridgeRejectionCause,
) -> mob_dsl::MobBridgeRejectionCause {
    use super::bridge_protocol::BridgeRejectionCause as Wire;
    use mob_dsl::MobBridgeRejectionCause as Mob;
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
        // Fail closed: an unknown future wire cause is treated as a hard
        // (fatal) rejection that no rebind can recover.
        _ => Mob::Internal,
    }
}

/// Render forked conversation messages as a text context block for the new member.
fn render_fork_context(
    source_member_id: &MeerkatId,
    messages: &[meerkat_core::types::Message],
) -> String {
    use meerkat_core::types::Message;

    let mut lines = Vec::new();
    lines.push(format!(
        "[Forked conversation context from member '{source_member_id}']"
    ));
    for msg in messages {
        match msg {
            Message::System(s) => {
                lines.push(format!("[system]: {}", s.content));
            }
            Message::SystemNotice(notice) => {
                lines.push(format!(
                    "[system_notice]: {}",
                    notice.model_projection_text()
                ));
            }
            Message::User(u) => {
                lines.push(format!("[user]: {}", u.text_content()));
            }
            Message::Assistant(a) => {
                if !a.content.is_empty() {
                    lines.push(format!("[assistant]: {}", a.content));
                }
            }
            Message::BlockAssistant(ba) => {
                // Both `Text` (display) and `Transcript` (spoken) lanes
                // project to the rendered text stream; supervisor sees the
                // assistant's full visible output regardless of lane.
                let text: String = ba
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        meerkat_core::types::AssistantBlock::Text { text, .. }
                        | meerkat_core::types::AssistantBlock::Transcript { text, .. } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    lines.push(format!("[assistant]: {text}"));
                }
            }
            Message::ToolResults { results, .. } => {
                for tr in results {
                    let text: String = tr
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            meerkat_core::types::ContentBlock::Text { text, .. } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !text.is_empty() {
                        let preview = if text.len() > 200 {
                            // Find a valid UTF-8 char boundary at or before byte 200
                            let end = text
                                .char_indices()
                                .map(|(i, _)| i)
                                .take_while(|&i| i <= 200)
                                .last()
                                .unwrap_or(0);
                            format!("{}...", &text[..end])
                        } else {
                            text
                        };
                        lines.push(format!("[tool_result({})]: {preview}", tr.tool_use_id));
                    }
                }
            }
        }
    }
    lines.push("[End of forked context]".to_string());
    lines.join("\n")
}

pub(super) struct PendingSpawn {
    pub(super) profile_name: ProfileName,
    pub(super) agent_identity: MeerkatId,
    pub(super) admitted_bridge_session_id: SessionId,
    pub(super) prompt: ContentInput,
    pub(super) initial_turn_prompt: Option<ContentInput>,
    pub(super) runtime_mode: crate::MobRuntimeMode,
    pub(super) labels: std::collections::BTreeMap<String, String>,
    pub(super) owner_bridge_session_id: Option<SessionId>,
    pub(super) auto_wire_parent: bool,
    /// Peer wiring to restore after respawn completes.
    pub(super) restore_wiring: Option<RestoreWiringPlan>,
    /// Effective profile override from `SpawnTooling::Profile` resolution.
    /// Persisted in the roster so respawn/restore can use it.
    pub(super) effective_profile_override: Option<crate::profile::Profile>,
    pub(super) authorized_profile_material: AuthorizedSpawnProfileMaterial,
    pub(super) continuity_intent: super::handle::SpawnContinuityIntent,
    pub(super) progress: Arc<std::sync::Mutex<PendingSpawnProgress>>,
    pub(super) reply_tx: oneshot::Sender<Result<super::handle::MemberSpawnReceipt, MobError>>,
}

#[derive(Debug, Default)]
pub(super) struct PendingSpawnProgress {
    pub(super) bridge_session_id: Option<meerkat_core::types::SessionId>,
    pub(super) operation_id: Option<meerkat_core::ops::OperationId>,
}

#[derive(Clone, Debug)]
pub(super) struct PendingSpawnCleanupAnchor {
    spawn_ticket: u64,
    agent_identity: MeerkatId,
    session_id: meerkat_core::types::SessionId,
    operation_id: meerkat_core::ops::OperationId,
    reason: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RestoreWiringPlan {
    local_peers: Vec<MeerkatId>,
    external_peers: Vec<TrustedPeerDescriptor>,
}

struct RespawnSnapshot {
    profile_name: ProfileName,
    runtime_mode: crate::MobRuntimeMode,
    labels: std::collections::BTreeMap<String, String>,
    old_runtime_id: crate::ids::AgentRuntimeId,
    old_fence_token: crate::ids::FenceToken,
    /// Generation of the member being respawned.
    generation: crate::ids::Generation,
    restore_wiring: RestoreWiringPlan,
    /// Runtime binding extracted from the old roster entry's member_ref.
    /// Preserves real external identity across respawns.
    binding: crate::RuntimeBinding,
    /// Effective profile override persisted in the roster.
    /// Used on respawn to avoid re-resolving from the definition.
    effective_profile_override: Option<crate::profile::Profile>,
    /// The old member is already in a partial-retire state and respawn should
    /// retry cleanup instead of re-admitting the original Respawn transition.
    cleanup_retry: bool,
}

struct FinalizeSpawnOutcome {
    receipt: super::handle::MemberSpawnReceipt,
    failed_restore_peer_ids: Vec<RespawnTopologyPeerId>,
}

struct RespawnTopologyRestoreResolution {
    result: mob_dsl::RespawnTopologyRestoreResultKind,
    failed_peer_ids: Vec<RespawnTopologyPeerId>,
}

#[cfg(not(target_arch = "wasm32"))]
struct RemoteDestroyOutcome {
    identity: AgentIdentity,
    force_destroyed: bool,
    orphaned: bool,
    errors: Vec<String>,
}

struct RuntimeMetadataSnapshot {
    supervisor: Option<crate::store::SupervisorAuthorityRecord>,
    external_binding_overlays: Vec<crate::store::ExternalBindingOverlayRecord>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedRevokeCleanupFailure {
    CommsPeerNotFound,
    CommsPeerOffline,
    CommsAdmissionDropped {
        reason: meerkat_core::comms::AdmissionDropReason,
    },
    BridgeRejected {
        cause: super::bridge_protocol::BridgeRejectionCause,
    },
}

// ---------------------------------------------------------------------------
// MobActor
// ---------------------------------------------------------------------------

/// The actor that processes mob commands sequentially.
///
/// Owns all mutable state. Runs in a dedicated tokio task.
/// All mutations go through here; reads bypass via shared `Arc` state.
pub(super) struct MobActor {
    pub(super) definition: Arc<MobDefinition>,
    pub(super) roster: Arc<RwLock<RosterAuthority>>,
    pub(super) events: Arc<dyn MobEventStore>,
    pub(super) run_store: Arc<dyn MobRunStore>,
    pub(super) provisioner: Arc<dyn MobProvisioner>,
    pub(super) flow_engine: FlowEngine,
    /// Whether this mob's definition declares an orchestrator.
    /// Gates orchestrator-specific transitions and notification fan-out.
    pub(super) has_orchestrator: bool,
    pub(super) run_tasks: BTreeMap<RunId, tokio::task::JoinHandle<()>>,
    pub(super) run_cancel_tokens: BTreeMap<RunId, (tokio_util::sync::CancellationToken, FlowId)>,
    pub(super) flow_streams:
        Arc<tokio::sync::Mutex<BTreeMap<RunId, mpsc::Sender<meerkat_core::ScopedAgentEvent>>>>,
    pub(super) command_tx: mpsc::Sender<MobCommand>,
    pub(super) tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    pub(super) default_llm_client: Option<Arc<dyn LlmClient>>,
    pub(super) retired_event_index: Arc<RwLock<HashSet<String>>>,
    pub(super) autonomous_initial_turns:
        Arc<tokio::sync::Mutex<BTreeMap<MeerkatId, InitialTurnHandle>>>,
    pub(super) next_spawn_ticket: u64,
    /// Monotonically increasing fence token counter.
    /// Each spawn/respawn/reset issues a strictly newer token.
    /// Uses `AtomicU64` so `&self` methods (batch finalization) can issue tokens.
    pub(super) next_fence_token: std::sync::atomic::AtomicU64,
    pub(super) pending_spawns: PendingSpawnLineage,
    pub(super) pending_spawn_cleanup_anchors: BTreeMap<u64, PendingSpawnCleanupAnchor>,
    pub(super) edge_locks: Arc<super::edge_locks::EdgeLockRegistry>,
    pub(super) lifecycle_tasks: tokio::task::JoinSet<()>,
    pub(super) next_peer_delivery_ticket: PeerDeliveryId,
    pub(super) peer_delivery_tasks: tokio::task::JoinSet<PeerDeliveryCompletion>,
    pub(super) peer_delivery_inflight: BTreeMap<PeerDeliveryId, PeerDeliveryInflight>,
    pub(super) peer_delivery_permits: Arc<tokio::sync::Semaphore>,
    pub(super) session_service: Arc<dyn MobSessionService>,
    #[cfg(feature = "runtime-adapter")]
    pub(super) runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    pub(super) restore_diagnostics:
        Arc<RwLock<HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic>>>,
    pub(super) runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    pub(super) supervisor_bridge: Arc<super::MobSupervisorBridge>,
    pub(super) spawn_policy: Arc<super::spawn_policy::SpawnPolicyService>,
    pub(super) dsl_authority: mob_dsl::MobMachineAuthority,
    pub(super) dsl_topology_epoch: Arc<std::sync::atomic::AtomicU64>,
    pub(super) dsl_authority_owner_token: Arc<dyn std::any::Any + Send + Sync>,
    /// Read-only MobMachine state projection for handle-side status/list
    /// surfaces. The actor is the sole writer; handles can borrow the latest
    /// state without enqueueing behind long shell cleanup work.
    pub(super) machine_state_watch_tx: tokio::sync::watch::Sender<mob_dsl::MobMachineState>,
    /// Terminal-phase projection for external observers. Written by the
    /// actor after every DSL phase transition and once more right before
    /// the actor task exits. `MobHandle::status()` falls back to this
    /// `watch` receiver when the command channel has closed (actor has
    /// exited post-Shutdown/Destroy). The watch is an explicit dogma-#13
    /// projection: the actor owns the sole writer, external handles hold
    /// read-only receivers, and the source of truth remains the DSL
    /// authority inside the actor.
    pub(super) phase_watch_tx: tokio::sync::watch::Sender<MobState>,
    pub(super) default_external_tools_provider: Option<crate::ExternalToolsProvider>,
    pub(super) spawn_member_customizer: Option<Arc<dyn super::SpawnMemberCustomizer>>,
    pub(super) realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
    /// Typed composition binding for the `meerkat_mob_seam` composition
    /// (wave-c C-6p). Every routed effect emitted by the mob DSL travels
    /// through `CompositionDispatcher::dispatch` via this binding rather
    /// than through direct peer / provisioner calls. Construction uses
    /// `CompositionBinding::Standalone` by default (test / ephemeral
    /// path); production surface assembly swaps in
    /// `CompositionBinding::Wired(Arc<dyn CompositionDispatcher<...>>)`.
    pub(super) composition_binding: super::composition::MobCompositionBinding,
    /// Routed seam-effects queued by sync `apply_dsl_input` /
    /// `apply_dsl_signal` calls. Drained by `flush_routed_effects` at
    /// every command-loop boundary and after each command handler; the
    /// first dispatch failure surfaces as a typed `MobError`, no silent
    /// drops.
    pub(super) pending_routed_effects: Vec<super::composition::MobSeamEffect>,
    pub(super) destroy_cleanup_active: bool,
}

struct AuthorizedPeerOnlyBind {
    peer: TrustedPeerDescriptor,
    response: super::bridge_protocol::BridgeBindResponse,
}

impl MobActor {
    fn peer_only_member_control_error(
        runtime_mode: crate::MobRuntimeMode,
        action: &str,
    ) -> MobError {
        MobError::UnsupportedForMode {
            mode: runtime_mode,
            reason: format!("{action} is not supported for peer-only members in phase 1"),
        }
    }

    fn peer_only_spec_from_parts(
        peer_id: &str,
        address: &str,
        context: &'static str,
        pubkey: Option<[u8; 32]>,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let peer_name = address
            .strip_prefix("inproc://")
            .map(|value| value.split('?').next().unwrap_or(value).to_string())
            .unwrap_or_else(|| format!("mob_member/backend_peer/{peer_id}"));
        let pubkey = pubkey.ok_or_else(|| {
            MobError::WiringError(format!(
                "{context}: peer-only runtime spec for '{peer_name}' requires pubkey"
            ))
        })?;
        let result = TrustedPeerDescriptor::unsigned_with_pubkey(
            peer_name,
            peer_id.to_string(),
            pubkey,
            address.to_string(),
        );
        result.map_err(|error| {
            MobError::WiringError(format!(
                "{context}: invalid peer-only runtime spec: {error}"
            ))
        })
    }

    fn peer_only_spec_for_binding(
        binding: &crate::RuntimeBinding,
        context: &'static str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        match binding {
            crate::RuntimeBinding::External {
                peer_id,
                address,
                pubkey,
                ..
            } => Self::peer_only_spec_from_parts(peer_id, address, context, *pubkey),
            crate::RuntimeBinding::Session => Err(MobError::Internal(format!(
                "{context}: peer-only runtime spec requested for session binding"
            ))),
        }
    }

    fn peer_only_spec_from_member_endpoint(
        endpoint: &mob_dsl::MemberPeerEndpoint,
        context: &'static str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        TrustedPeerDescriptor::unsigned_with_pubkey(
            endpoint.name.0.clone(),
            endpoint.peer_id.0.clone(),
            endpoint.signing_key.0,
            endpoint.address.0.clone(),
        )
        .map_err(|error| {
            MobError::WiringError(format!(
                "{context}: invalid MobMachine member peer endpoint authority: {error}"
            ))
        })
    }

    fn member_peer_rebind_endpoint_from_transition(
        transition: &mob_dsl::MobMachineTransition,
        agent_identity: &mob_dsl::AgentIdentity,
        context: &'static str,
    ) -> Result<mob_dsl::MemberPeerEndpoint, MobError> {
        let mut authorized = None;
        for effect in transition.effects() {
            if let mob_dsl::MobMachineEffect::MemberPeerRebindAuthorized {
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
                        "{context}: duplicate generated member peer rebind authority for '{}'",
                        agent_identity.0
                    )));
                }
            }
        }
        authorized.ok_or_else(|| {
            MobError::WiringError(format!(
                "{context}: missing generated member peer rebind authority for '{}'",
                agent_identity.0
            ))
        })
    }

    fn trusted_peer_removal_key(peer: &TrustedPeerDescriptor) -> String {
        peer.peer_id.to_string()
    }

    fn supervisor_publish_authority(
        obligation: &meerkat_runtime::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation,
    ) -> Result<CommsTrustMutationAuthority, String> {
        meerkat_runtime::protocol_supervisor_trust_publish::publish_authority_for_peer(
            obligation,
            obligation.peer_id(),
        )
    }

    fn supervisor_publish_cleanup_authority(
        obligation: &meerkat_runtime::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation,
    ) -> Result<CommsTrustMutationAuthority, String> {
        meerkat_runtime::protocol_supervisor_trust_publish::cleanup_authority_for_peer(
            obligation,
            obligation.peer_id(),
        )
    }

    fn supervisor_revoke_authority(
        obligation: &meerkat_runtime::protocol_supervisor_trust_revoke::SupervisorTrustRevokeObligation,
    ) -> Result<CommsTrustMutationAuthority, String> {
        meerkat_runtime::protocol_supervisor_trust_revoke::revoke_authority_for_peer(
            obligation,
            obligation.peer_id(),
        )
    }

    fn unexpected_trust_mutation_result(
        operation: &'static str,
        result: CommsTrustMutationResult,
    ) -> SendError {
        SendError::Internal(format!(
            "{operation} returned unexpected trust mutation result: {result:?}"
        ))
    }

    async fn bind_generated_mob_trust_owner_for_authority(
        &self,
        comms: &(dyn CoreCommsRuntime + '_),
        authority: &CommsTrustMutationAuthority,
    ) -> Result<(), SendError> {
        Self::bind_generated_mob_trust_owner_for_authority_with_token(
            comms,
            authority,
            &self.dsl_authority.generated_authority_owner_token(),
        )
        .await
    }

    async fn bind_generated_mob_trust_owner_for_authority_with_token(
        comms: &(dyn CoreCommsRuntime + '_),
        authority: &CommsTrustMutationAuthority,
        owner_token: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        if authority.is_mob_machine_source() {
            comms
                .install_generated_mob_trust_owner(Arc::clone(owner_token))
                .await?;
        }
        Ok(())
    }

    async fn apply_trusted_peer_add(
        &self,
        comms: &(dyn CoreCommsRuntime + '_),
        peer: TrustedPeerDescriptor,
        authority: CommsTrustMutationAuthority,
    ) -> Result<(), SendError> {
        self.apply_trusted_peer_add_report(comms, peer, authority)
            .await?;
        Ok(())
    }

    async fn apply_trusted_peer_add_report(
        &self,
        comms: &(dyn CoreCommsRuntime + '_),
        peer: TrustedPeerDescriptor,
        authority: CommsTrustMutationAuthority,
    ) -> Result<bool, SendError> {
        Self::apply_trusted_peer_add_with_owner_token_report(
            comms,
            peer,
            authority,
            &self.dsl_authority.generated_authority_owner_token(),
        )
        .await
    }

    async fn apply_trusted_peer_add_with_owner_token(
        comms: &(dyn CoreCommsRuntime + '_),
        peer: TrustedPeerDescriptor,
        authority: CommsTrustMutationAuthority,
        owner_token: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        Self::apply_trusted_peer_add_with_owner_token_report(comms, peer, authority, owner_token)
            .await?;
        Ok(())
    }

    async fn apply_trusted_peer_add_with_owner_token_report(
        comms: &(dyn CoreCommsRuntime + '_),
        peer: TrustedPeerDescriptor,
        authority: CommsTrustMutationAuthority,
        owner_token: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<bool, SendError> {
        Self::bind_generated_mob_trust_owner_for_authority_with_token(
            comms,
            &authority,
            owner_token,
        )
        .await?;
        match comms
            .apply_trust_mutation(CommsTrustMutation::AddTrustedPeer { peer, authority })
            .await?
        {
            CommsTrustMutationResult::Added { created } => Ok(created),
            result => Err(Self::unexpected_trust_mutation_result(
                "add trusted peer",
                result,
            )),
        }
    }

    async fn apply_trusted_peer_remove(
        &self,
        comms: &(dyn CoreCommsRuntime + '_),
        peer_id: String,
        authority: CommsTrustMutationAuthority,
    ) -> Result<bool, SendError> {
        Self::apply_trusted_peer_remove_with_owner_token(
            comms,
            peer_id,
            authority,
            &self.dsl_authority.generated_authority_owner_token(),
        )
        .await
    }

    async fn apply_trusted_peer_remove_with_owner_token(
        comms: &(dyn CoreCommsRuntime + '_),
        peer_id: String,
        authority: CommsTrustMutationAuthority,
        owner_token: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<bool, SendError> {
        Self::bind_generated_mob_trust_owner_for_authority_with_token(
            comms,
            &authority,
            owner_token,
        )
        .await?;
        match comms
            .apply_trust_mutation(CommsTrustMutation::RemoveTrustedPeer { peer_id, authority })
            .await?
        {
            CommsTrustMutationResult::Removed { removed } => Ok(removed),
            result => Err(Self::unexpected_trust_mutation_result(
                "remove trusted peer",
                result,
            )),
        }
    }

    async fn apply_private_trusted_peer_add(
        &self,
        comms: &(dyn CoreCommsRuntime + '_),
        peer: TrustedPeerDescriptor,
        authority: CommsTrustMutationAuthority,
    ) -> Result<(), SendError> {
        self.bind_generated_mob_trust_owner_for_authority(comms, &authority)
            .await?;
        match comms
            .apply_trust_mutation(CommsTrustMutation::AddPrivateTrustedPeer { peer, authority })
            .await?
        {
            CommsTrustMutationResult::Added { .. } => Ok(()),
            result => Err(Self::unexpected_trust_mutation_result(
                "add private trusted peer",
                result,
            )),
        }
    }

    async fn apply_private_trusted_peer_remove(
        &self,
        comms: &(dyn CoreCommsRuntime + '_),
        peer_id: String,
        authority: CommsTrustMutationAuthority,
    ) -> Result<bool, SendError> {
        self.bind_generated_mob_trust_owner_for_authority(comms, &authority)
            .await?;
        match comms
            .apply_trust_mutation(CommsTrustMutation::RemovePrivateTrustedPeer {
                peer_id,
                authority,
            })
            .await?
        {
            CommsTrustMutationResult::Removed { removed } => Ok(removed),
            result => Err(Self::unexpected_trust_mutation_result(
                "remove private trusted peer",
                result,
            )),
        }
    }

    fn supervisor_spec_for_authority(
        mob_id: &crate::MobId,
        authority: &crate::store::SupervisorAuthorityRecord,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let participant_name = format!("{mob_id}/__mob_supervisor__");
        let public_key = authority.keypair().public_key();
        TrustedPeerDescriptor::unsigned_with_pubkey(
            participant_name.clone(),
            authority.public_peer_id.clone(),
            *public_key.as_bytes(),
            format!("inproc://{participant_name}"),
        )
        .map_err(|error| MobError::WiringError(format!("invalid supervisor spec: {error}")))
    }

    async fn install_supervisor_private_trust_for_session(
        &self,
        session_id: &SessionId,
        comms: &Arc<dyn CoreCommsRuntime>,
        previous_private_trust_removal_key: Option<&str>,
    ) -> Result<SupervisorPrivateTrustInstall, MobError> {
        let authority = self.supervisor_bridge.authority().await;
        let spec = Self::supervisor_spec_for_authority(&self.definition.id, &authority)?;
        #[cfg(target_arch = "wasm32")]
        {
            Box::pin(self.install_supervisor_private_trust_for_session_authority(
                session_id,
                comms,
                &authority,
                spec,
                previous_private_trust_removal_key,
            ))
            .await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.install_supervisor_private_trust_for_session_authority(
                session_id,
                comms,
                &authority,
                spec,
                previous_private_trust_removal_key,
            )
            .await
        }
    }

    async fn install_supervisor_private_trust_for_session_authority(
        &self,
        session_id: &SessionId,
        comms: &Arc<dyn CoreCommsRuntime>,
        authority: &crate::store::SupervisorAuthorityRecord,
        spec: TrustedPeerDescriptor,
        previous_private_trust_removal_key: Option<&str>,
    ) -> Result<SupervisorPrivateTrustInstall, MobError> {
        #[cfg(feature = "runtime-adapter")]
        let Some(adapter) = self.runtime_adapter.as_ref() else {
            return Err(MobError::Internal(format!(
                "cannot publish supervisor private trust for session '{session_id}': runtime adapter unavailable"
            )));
        };
        #[cfg(not(feature = "runtime-adapter"))]
        let _ = session_id;
        #[cfg(not(feature = "runtime-adapter"))]
        {
            return Err(MobError::Internal(
                "cannot publish supervisor private trust without runtime adapter".to_string(),
            ));
        }

        #[cfg(feature = "runtime-adapter")]
        {
            use meerkat_runtime::protocol_supervisor_trust_publish;

            adapter
                .stage_local_endpoint_for_comms_runtime(session_id, comms.as_ref())
                .await
                .map_err(|error| {
                    MobError::WiringError(format!(
                        "supervisor private trust local endpoint rejected for session '{session_id}': {error}"
                    ))
                })?;

            let next_name = spec.name.as_str().to_owned();
            let next_peer_id = spec.peer_id.as_str().to_owned();
            let next_address = spec.address.to_string();
            let next_signing_public_key =
                meerkat_runtime::comms_drain::encode_supervisor_signing_public_key(spec.pubkey);
            let next_epoch = authority.epoch;
            let previous = adapter.supervisor_binding(session_id).await;
            let already_bound = matches!(
                &previous,
                meerkat_runtime::meerkat_machine::SupervisorBinding::Bound {
                    name,
                    peer_id,
                    address,
                    signing_public_key,
                    epoch,
                } if name == &next_name
                    && peer_id == &next_peer_id
                    && address == &next_address
                    && signing_public_key == &next_signing_public_key
                    && *epoch == next_epoch
            );

            let previous_peer_is_different = matches!(
                &previous,
                meerkat_runtime::meerkat_machine::SupervisorBinding::Bound { peer_id, .. }
                    if peer_id != &next_peer_id
            );
            if previous_peer_is_different {
                let (previous_peer_id, previous_epoch) = match &previous {
                    meerkat_runtime::meerkat_machine::SupervisorBinding::Bound {
                        peer_id,
                        epoch,
                        ..
                    } => (peer_id.clone(), *epoch),
                    _ => unreachable!("previous_peer_is_different only matches Bound"),
                };
                let revoke_transition = adapter
                    .stage_supervisor_revoke(session_id, previous_peer_id.clone(), previous_epoch)
                    .await
                    .map_err(|error| {
                        MobError::WiringError(format!(
                            "previous supervisor private trust revoke rejected for session '{session_id}': {error}"
                        ))
                    })?;
                let revoke_freshness = adapter
                    .supervisor_trust_revoke_freshness_authority(session_id)
                    .await
                    .map_err(|error| {
                        MobError::WiringError(format!(
                            "previous supervisor private trust revoke freshness unavailable for session '{session_id}': {error}"
                        ))
                    })?;
                let revoke_obligation =
                    meerkat_runtime::protocol_supervisor_trust_revoke::extract_obligations_with_freshness(
                        &revoke_transition,
                        revoke_freshness,
                    )
                    .into_iter()
                    .find(|obligation| {
                        obligation.peer_id() == &previous_peer_id
                            && obligation.epoch() == previous_epoch
                    })
                    .ok_or_else(|| {
                        MobError::WiringError(format!(
                            "previous supervisor private trust revoke for session '{session_id}' produced no generated revoke obligation"
                        ))
                    })?;
                let previous_removal_key = previous_private_trust_removal_key
                    .map(str::to_string)
                    .unwrap_or_else(|| revoke_obligation.peer_id().clone());
                if let Err(error) = self
                    .apply_private_trusted_peer_remove(
                        comms.as_ref(),
                        previous_removal_key,
                        Self::supervisor_revoke_authority(&revoke_obligation)
                            .map_err(MobError::WiringError)?,
                    )
                    .await
                {
                    let feedback = adapter
                        .stage_supervisor_trust_revoke_failed(
                            session_id,
                            revoke_obligation.peer_id().clone(),
                            revoke_obligation.epoch(),
                            error.to_string(),
                        )
                        .await;
                    let mut reason = format!(
                        "previous supervisor private trust removal failed for session '{session_id}': {error}"
                    );
                    if let Err(feedback_error) = feedback {
                        reason.push_str(&format!("; revoke feedback failed: {feedback_error}"));
                    }
                    return Err(MobError::WiringError(reason));
                }
                adapter
                    .stage_supervisor_trust_revoked(
                        session_id,
                        revoke_obligation.peer_id().clone(),
                        revoke_obligation.epoch(),
                    )
                    .await
                    .map_err(|error| {
                        MobError::WiringError(format!(
                            "previous supervisor private trust revoke feedback rejected for session '{session_id}': {error}"
                        ))
                    })?;
            }

            let stage_transition = if already_bound {
                adapter
                    .stage_supervisor_trust_publish_request(
                        session_id,
                        next_name.clone(),
                        next_peer_id.clone(),
                        next_address.clone(),
                        next_signing_public_key.clone(),
                        next_epoch,
                    )
                    .await
                    .map_err(|error| {
                        MobError::WiringError(format!(
                            "supervisor private trust publish request rejected for session '{session_id}': {error}"
                    ))
                })?
            } else if previous_peer_is_different {
                Self::stage_supervisor_bind_for_private_trust(
                    adapter,
                    session_id,
                    next_name.clone(),
                    next_peer_id.clone(),
                    next_address.clone(),
                    next_signing_public_key.clone(),
                    next_epoch,
                )
                .await
                .map_err(|error| {
                    MobError::WiringError(format!(
                        "supervisor private trust bind rejected for session '{session_id}': {error}"
                    ))
                })?
            } else {
                match &previous {
                    meerkat_runtime::meerkat_machine::SupervisorBinding::Unbound => {
                        Self::stage_supervisor_bind_for_private_trust(
                            adapter,
                            session_id,
                            next_name.clone(),
                            next_peer_id.clone(),
                            next_address.clone(),
                            next_signing_public_key.clone(),
                            next_epoch,
                        )
                        .await
                        .map_err(|error| {
                            MobError::WiringError(format!(
                                "supervisor private trust bind rejected for session '{session_id}': {error}"
                            ))
                        })?
                    }
                    meerkat_runtime::meerkat_machine::SupervisorBinding::Bound { .. } => {
                        adapter
                            .stage_supervisor_authorize(
                                session_id,
                                next_name.clone(),
                                next_peer_id.clone(),
                                next_address.clone(),
                                next_signing_public_key.clone(),
                                next_epoch,
                            )
                            .await
                            .map_err(|error| {
                                MobError::WiringError(format!(
                                    "supervisor private trust rotation rejected for session '{session_id}': {error}"
                                ))
                            })?
                    }
                    _ => {
                        return Err(MobError::WiringError(format!(
                            "supervisor private trust publication for session '{session_id}' saw an unknown supervisor binding variant"
                        )));
                    }
                }
            };
            let publish_freshness = adapter
                .supervisor_trust_publish_freshness_authority(session_id)
                .await
                .map_err(|error| {
                    MobError::WiringError(format!(
                        "supervisor private trust publish freshness unavailable for session '{session_id}': {error}"
                    ))
                })?;
            let obligations = protocol_supervisor_trust_publish::extract_obligations_with_freshness(
                &stage_transition,
                publish_freshness,
            );
            let publish_obligation = match obligations.as_slice() {
                [obligation] => obligation.clone(),
                [] => {
                    return Err(MobError::WiringError(format!(
                        "supervisor private trust publication for session '{session_id}' produced no generated publish obligation"
                    )));
                }
                _ => {
                    return Err(MobError::WiringError(format!(
                        "supervisor private trust publication for session '{session_id}' produced multiple generated publish obligations"
                    )));
                }
            };
            if publish_obligation.name() != &next_name
                || publish_obligation.peer_id() != &next_peer_id
                || publish_obligation.address() != &next_address
                || publish_obligation.signing_public_key().as_deref()
                    != Some(next_signing_public_key.as_str())
                || publish_obligation.epoch() != next_epoch
            {
                return Err(MobError::WiringError(format!(
                    "supervisor private trust publication for session '{session_id}' generated obligation did not match the staged supervisor binding"
                )));
            }
            let publish_spec =
                meerkat_runtime::comms_drain::trusted_peer_descriptor_from_supervisor_publish_obligation(
                    &publish_obligation,
                )
                .map_err(|error| {
                    MobError::WiringError(format!(
                        "supervisor private trust publication for session '{session_id}' generated invalid trust descriptor: {error}"
                    ))
                })?;
            let publish_peer_id = publish_obligation.peer_id().clone();
            let publish_epoch = publish_obligation.epoch();
            let publish_removal_key = Self::trusted_peer_removal_key(&publish_spec);
            let publish_cleanup_authority =
                Self::supervisor_publish_cleanup_authority(&publish_obligation)
                    .map_err(MobError::WiringError)?;
            let rollback_binding = previous.clone();

            if let Err(error) = self
                .apply_private_trusted_peer_add(
                    comms.as_ref(),
                    publish_spec.clone(),
                    Self::supervisor_publish_authority(&publish_obligation)
                        .map_err(MobError::WiringError)?,
                )
                .await
            {
                let _ = adapter
                    .stage_supervisor_trust_publish_failed(
                        session_id,
                        publish_peer_id.clone(),
                        publish_epoch,
                        error.to_string(),
                    )
                    .await;
                if !already_bound {
                    self.cleanup_supervisor_private_trust_publish_attempt(
                        session_id,
                        comms,
                        publish_cleanup_authority.clone(),
                        publish_removal_key.clone(),
                        "failed to clean up supervisor private trust after publish add failure",
                    )
                    .await;
                }
                let rollback = if already_bound {
                    Ok(())
                } else {
                    self.rollback_supervisor_private_trust_binding(
                        adapter,
                        session_id,
                        comms,
                        &rollback_binding,
                        &publish_peer_id,
                        publish_epoch,
                    )
                    .await
                };
                let mut reason = format!(
                    "supervisor private trust publication failed for session '{session_id}': {error}"
                );
                if let Err(rollback_error) = rollback {
                    reason.push_str(&format!("; rollback failed: {rollback_error}"));
                }
                return Err(MobError::WiringError(reason));
            }

            if let Err(error) = Self::stage_supervisor_trust_published_for_private_trust(
                adapter,
                session_id,
                publish_peer_id.clone(),
                publish_epoch,
            )
            .await
            {
                if !already_bound {
                    self.cleanup_supervisor_private_trust_publish_attempt(
                        session_id,
                        comms,
                        publish_cleanup_authority,
                        publish_removal_key.clone(),
                        "failed to clean up supervisor private trust after rejected publish ack",
                    )
                    .await;
                }
                let rollback = if already_bound {
                    Ok(())
                } else {
                    self.rollback_supervisor_private_trust_binding(
                        adapter,
                        session_id,
                        comms,
                        &rollback_binding,
                        &publish_peer_id,
                        publish_epoch,
                    )
                    .await
                };
                let mut reason = format!(
                    "supervisor private trust publication ack rejected for session '{session_id}': {error}"
                );
                if let Err(rollback_error) = rollback {
                    reason.push_str(&format!("; rollback failed: {rollback_error}"));
                }
                return Err(MobError::WiringError(reason));
            }

            Ok(SupervisorPrivateTrustInstall {
                peer_id: next_peer_id,
                epoch: next_epoch,
                removal_key: publish_removal_key,
            })
        }
    }

    #[cfg(feature = "runtime-adapter")]
    async fn stage_supervisor_trust_published_for_private_trust(
        adapter: &Arc<meerkat_runtime::MeerkatMachine>,
        session_id: &SessionId,
        peer_id: String,
        epoch: u64,
    ) -> Result<(), meerkat_runtime::meerkat_machine::SupervisorBindingStageError> {
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = Arc::clone(adapter);
            let session_id = session_id.clone();
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let result = adapter
                    .stage_supervisor_trust_published(&session_id, peer_id, epoch)
                    .await;
                let _ = reply_tx.send(result);
            });
            reply_rx.await.map_err(|_| {
                meerkat_runtime::meerkat_machine::SupervisorBindingStageError::SessionRegistryBusy
            })?
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            adapter
                .stage_supervisor_trust_published(session_id, peer_id, epoch)
                .await
        }
    }

    #[cfg(feature = "runtime-adapter")]
    async fn stage_supervisor_bind_for_private_trust(
        adapter: &Arc<meerkat_runtime::MeerkatMachine>,
        session_id: &SessionId,
        name: String,
        peer_id: String,
        address: String,
        signing_public_key: String,
        epoch: u64,
    ) -> Result<
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineTransition,
        meerkat_runtime::meerkat_machine::SupervisorBindingStageError,
    > {
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = Arc::clone(adapter);
            let session_id = session_id.clone();
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let result = adapter
                    .stage_supervisor_bind(
                        &session_id,
                        name,
                        peer_id,
                        address,
                        signing_public_key,
                        epoch,
                    )
                    .await;
                let _ = reply_tx.send(result);
            });
            reply_rx.await.map_err(|_| {
                meerkat_runtime::meerkat_machine::SupervisorBindingStageError::SessionRegistryBusy
            })?
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            adapter
                .stage_supervisor_bind(
                    session_id,
                    name,
                    peer_id,
                    address,
                    signing_public_key,
                    epoch,
                )
                .await
        }
    }

    async fn cleanup_supervisor_private_trust_publish_attempt(
        &self,
        session_id: &SessionId,
        comms: &Arc<dyn CoreCommsRuntime>,
        authority: CommsTrustMutationAuthority,
        removal_key: String,
        context: &'static str,
    ) {
        if let Err(error) = self
            .apply_private_trusted_peer_remove(comms.as_ref(), removal_key, authority)
            .await
        {
            tracing::warn!(
                %session_id,
                %error,
                context,
                "failed to clean up supervisor private trust publish attempt"
            );
        }
    }

    async fn cleanup_supervisor_private_trust_for_session(
        &self,
        session_id: &SessionId,
        comms: &Arc<dyn CoreCommsRuntime>,
        install: &SupervisorPrivateTrustInstall,
    ) {
        #[cfg(feature = "runtime-adapter")]
        if let Some(adapter) = self.runtime_adapter.as_ref() {
            if let Err(error) = adapter
                .stage_local_endpoint_for_comms_runtime(session_id, comms.as_ref())
                .await
            {
                tracing::warn!(
                    %session_id,
                    peer_id = %install.peer_id,
                    epoch = install.epoch,
                    %error,
                    "failed to stage local endpoint for supervisor private trust cleanup"
                );
                return;
            }
            let transition = match adapter
                .stage_supervisor_revoke(session_id, install.peer_id.clone(), install.epoch)
                .await
            {
                Ok(transition) => transition,
                Err(error) => {
                    tracing::warn!(
                        %session_id,
                        peer_id = %install.peer_id,
                        epoch = install.epoch,
                        %error,
                        "failed to stage supervisor private trust cleanup"
                    );
                    return;
                }
            };
            let revoke_freshness = match adapter
                .supervisor_trust_revoke_freshness_authority(session_id)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    tracing::warn!(
                        %session_id,
                        peer_id = %install.peer_id,
                        epoch = install.epoch,
                        %error,
                        "failed to build generated supervisor private trust cleanup freshness"
                    );
                    return;
                }
            };
            let obligations =
                meerkat_runtime::protocol_supervisor_trust_revoke::extract_obligations_with_freshness(&transition, revoke_freshness);
            let Some(obligation) = obligations.into_iter().find(|obligation| {
                obligation.peer_id() == &install.peer_id && obligation.epoch() == install.epoch
            }) else {
                let reason =
                    "generated supervisor private trust cleanup effect was absent".to_string();
                let _ = adapter
                    .stage_supervisor_trust_revoke_failed(
                        session_id,
                        install.peer_id.clone(),
                        install.epoch,
                        reason.clone(),
                    )
                    .await;
                tracing::warn!(
                    %session_id,
                    peer_id = %install.peer_id,
                    epoch = install.epoch,
                    reason,
                    "failed to stage supervisor private trust cleanup"
                );
                return;
            };
            if let Err(error) = self.apply_private_trusted_peer_remove(
                comms.as_ref(),
                install.removal_key.clone(),
                match Self::supervisor_revoke_authority(&obligation) {
                    Ok(authority) => authority,
                    Err(error) => {
                        let _ = adapter
                            .stage_supervisor_trust_revoke_failed(
                                session_id,
                                obligation.peer_id().clone(),
                                obligation.epoch(),
                                error.clone(),
                            )
                            .await;
                        tracing::warn!(
                            %session_id,
                            peer_id = %install.peer_id,
                            epoch = install.epoch,
                            %error,
                            "failed to build generated supervisor private trust cleanup authority"
                        );
                        return;
                    }
                },
            )
            .await
            {
                let _ = adapter
                    .stage_supervisor_trust_revoke_failed(
                        session_id,
                        obligation.peer_id().clone(),
                        obligation.epoch(),
                        error.to_string(),
                    )
                    .await;
                tracing::warn!(
                    %session_id,
                    peer_id = %install.peer_id,
                    epoch = install.epoch,
                    %error,
                    "failed to clean up supervisor private trust"
                );
                return;
            }
            if let Err(error) = adapter
                .stage_supervisor_trust_revoked(
                    session_id,
                    obligation.peer_id().clone(),
                    obligation.epoch(),
                )
                .await
            {
                tracing::warn!(
                    %session_id,
                    peer_id = %install.peer_id,
                    epoch = install.epoch,
                    %error,
                    "failed to acknowledge supervisor private trust cleanup"
                );
            }
            return;
        }

        let _ = comms;
        tracing::warn!(
            %session_id,
            peer_id = %install.peer_id,
            epoch = install.epoch,
            "skipping supervisor private trust cleanup because generated runtime adapter authority is unavailable"
        );
    }

    #[cfg(feature = "runtime-adapter")]
    async fn rollback_supervisor_private_trust_binding(
        &self,
        adapter: &Arc<meerkat_runtime::MeerkatMachine>,
        session_id: &SessionId,
        comms: &Arc<dyn CoreCommsRuntime>,
        previous: &meerkat_runtime::meerkat_machine::SupervisorBinding,
        current_peer_id: &str,
        current_epoch: u64,
    ) -> Result<(), MobError> {
        adapter
            .stage_local_endpoint_for_comms_runtime(session_id, comms.as_ref())
            .await
            .map_err(|error| MobError::WiringError(error.to_string()))?;
        match previous {
            meerkat_runtime::meerkat_machine::SupervisorBinding::Unbound => {
                let transition = adapter
                    .stage_supervisor_revoke(session_id, current_peer_id.to_string(), current_epoch)
                    .await
                    .map_err(|error| MobError::WiringError(error.to_string()))?;
                let revoke_freshness = adapter
                    .supervisor_trust_revoke_freshness_authority(session_id)
                    .await
                    .map_err(|error| MobError::WiringError(error.to_string()))?;
                if let Some(obligation) =
                    meerkat_runtime::protocol_supervisor_trust_revoke::extract_obligations_with_freshness(
                        &transition,
                        revoke_freshness,
                    )
                    .into_iter()
                    .find(|obligation| {
                        obligation.peer_id().as_str() == current_peer_id
                            && obligation.epoch() == current_epoch
                    })
                {
                    adapter
                        .stage_supervisor_trust_revoked(
                            session_id,
                            obligation.peer_id().clone(),
                            obligation.epoch(),
                        )
                        .await
                        .map_err(|error| MobError::WiringError(error.to_string()))?;
                }
                Ok(())
            }
            meerkat_runtime::meerkat_machine::SupervisorBinding::Bound {
                name,
                peer_id,
                address,
                signing_public_key,
                epoch,
            } => {
                let current = adapter.supervisor_binding(session_id).await;
                let transition = match current {
                    meerkat_runtime::meerkat_machine::SupervisorBinding::Unbound => adapter
                        .stage_supervisor_bind(
                            session_id,
                            name.clone(),
                            peer_id.clone(),
                            address.clone(),
                            signing_public_key.clone(),
                            *epoch,
                        )
                        .await,
                    meerkat_runtime::meerkat_machine::SupervisorBinding::Bound { .. } => adapter
                        .stage_supervisor_authorize(
                            session_id,
                            name.clone(),
                            peer_id.clone(),
                            address.clone(),
                            signing_public_key.clone(),
                            *epoch,
                        )
                        .await,
                    other => {
                        return Err(MobError::WiringError(format!(
                            "supervisor private trust rollback for session '{session_id}' saw unsupported current binding {other:?}"
                        )));
                    }
                }
                .map_err(|error| MobError::WiringError(error.to_string()))?;
                let publish_freshness = adapter
                    .supervisor_trust_publish_freshness_authority(session_id)
                    .await
                    .map_err(|error| MobError::WiringError(error.to_string()))?;
                let obligation =
                    meerkat_runtime::protocol_supervisor_trust_publish::extract_obligations_with_freshness(
                        &transition,
                        publish_freshness,
                    )
                    .into_iter()
                    .find(|obligation| {
                        obligation.peer_id() == peer_id
                            && obligation.epoch() == *epoch
                            && obligation.signing_public_key().as_deref()
                                == Some(signing_public_key.as_str())
                    })
                    .ok_or_else(|| {
                        MobError::WiringError(format!(
                            "supervisor private trust rollback for session '{session_id}' produced no generated publish obligation"
                        ))
                    })?;
                let trusted_peer =
                    meerkat_runtime::comms_drain::trusted_peer_descriptor_from_supervisor_publish_obligation(
                        &obligation,
                    )
                    .map_err(MobError::WiringError)?;
                self.apply_private_trusted_peer_add(
                    comms.as_ref(),
                    trusted_peer,
                    Self::supervisor_publish_authority(&obligation)
                        .map_err(MobError::WiringError)?,
                )
                .await
                .map_err(|error| MobError::WiringError(error.to_string()))?;
                adapter
                    .stage_supervisor_trust_published(
                        session_id,
                        obligation.peer_id().clone(),
                        obligation.epoch(),
                    )
                    .await
                    .map_err(|error| MobError::WiringError(error.to_string()))?;
                Ok(())
            }
            _ => Err(MobError::WiringError(
                "unknown supervisor binding variant during rollback".to_string(),
            )),
        }
    }

    async fn bridge_supervisor_payload(
        &self,
    ) -> Result<super::bridge_protocol::BridgeSupervisorPayload, MobError> {
        let authority = self.supervisor_bridge.authority().await;
        self.supervisor_payload_for_authority(&authority)
    }

    fn supervisor_payload_for_authority(
        &self,
        authority: &crate::store::SupervisorAuthorityRecord,
    ) -> Result<super::bridge_protocol::BridgeSupervisorPayload, MobError> {
        let spec = Self::supervisor_spec_for_authority(&self.definition.id, authority)?;
        Ok(super::bridge_protocol::BridgeSupervisorPayload {
            supervisor: spec.into(),
            epoch: authority.epoch,
            protocol_version: authority.protocol_version,
        })
    }

    fn bridge_bootstrap_token_from_binding(
        binding: &crate::RuntimeBinding,
    ) -> Result<super::bridge_protocol::BridgeBootstrapToken, MobError> {
        match binding {
            crate::RuntimeBinding::External {
                address,
                bootstrap_token,
                ..
            } => bootstrap_token
                .as_ref()
                .filter(|token| !token.is_empty())
                .cloned()
                .ok_or_else(|| {
                    MobError::WiringError(format!(
                        "external runtime binding for '{address}' is missing typed bootstrap_token field"
                    ))
                }),
            crate::RuntimeBinding::Session => Err(MobError::Internal(
                "bridge bootstrap token requested for session binding".to_string(),
            )),
        }
    }

    fn authorize_existing_peer_binding_for_rebind(
        &mut self,
        binding: &crate::RuntimeBinding,
        context: &'static str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let crate::RuntimeBinding::External {
            peer_id,
            address,
            pubkey,
            ..
        } = binding
        else {
            return Err(MobError::Internal(format!(
                "{context}: peer-only rebind authority requested for session binding"
            )));
        };
        let canonical_address = super::bridge_protocol::canonicalize_bridge_address(address);
        let observed_peer =
            Self::peer_only_spec_from_parts(peer_id, &canonical_address, context, *pubkey)?;
        let affected_identities: BTreeSet<_> = self
            .dsl_authority
            .state()
            .member_peer_ids
            .iter()
            .filter(|(_, existing_peer_id)| existing_peer_id.0 == *peer_id)
            .map(|(identity, _)| identity.clone())
            .collect();
        if affected_identities.is_empty() {
            return Err(MobError::WiringError(format!(
                "{context}: peer-only rebind for '{peer_id}' requires MobMachine member peer authority"
            )));
        }
        let mut authorized_peer: Option<TrustedPeerDescriptor> = None;
        for identity in &affected_identities {
            let expected_peer_endpoint = self
                .dsl_authority
                .state()
                .member_peer_endpoints
                .get(identity)
                .cloned()
                .ok_or_else(|| {
                    MobError::WiringError(format!(
                        "{context}: peer-only rebind for '{}' lacks MobMachine endpoint authority",
                        identity.0
                    ))
                })?;
            let transition = self.apply_dsl_input_collect_transition(
                mob_dsl::MobMachineInput::AuthorizeMemberPeerRebind {
                    agent_identity: identity.clone(),
                    expected_peer_endpoint,
                },
                context,
            )?;
            let endpoint =
                Self::member_peer_rebind_endpoint_from_transition(&transition, identity, context)?;
            let peer = Self::peer_only_spec_from_member_endpoint(&endpoint, context)?;
            if peer.name != observed_peer.name
                || peer.peer_id != observed_peer.peer_id
                || peer.address != observed_peer.address
                || peer.pubkey != observed_peer.pubkey
            {
                return Err(MobError::WiringError(format!(
                    "{context}: observed peer-only rebind endpoint for '{peer_id}' is outside generated MobMachine authority"
                )));
            }
            if let Some(existing) = &authorized_peer
                && (existing.name != peer.name
                    || existing.peer_id != peer.peer_id
                    || existing.address != peer.address
                    || existing.pubkey != peer.pubkey)
            {
                return Err(MobError::WiringError(format!(
                    "{context}: generated MobMachine peer rebind authority disagrees across identities for '{peer_id}'"
                )));
            }
            authorized_peer = Some(peer);
        }
        authorized_peer.ok_or_else(|| {
            MobError::WiringError(format!(
                "{context}: peer-only rebind for '{peer_id}' produced no generated authority"
            ))
        })
    }

    async fn bind_peer_only_member_for_binding(
        &mut self,
        peer: &TrustedPeerDescriptor,
        binding: &crate::RuntimeBinding,
    ) -> Result<AuthorizedPeerOnlyBind, MobError> {
        let payload = self.bridge_supervisor_payload().await?;
        self.bind_peer_only_member_for_binding_with_payload(peer, binding, &payload)
            .await
    }

    async fn bind_peer_only_member_for_binding_with_payload(
        &mut self,
        peer: &TrustedPeerDescriptor,
        binding: &crate::RuntimeBinding,
        payload: &super::bridge_protocol::BridgeSupervisorPayload,
    ) -> Result<AuthorizedPeerOnlyBind, MobError> {
        let crate::RuntimeBinding::External {
            peer_id,
            address: _,
            bootstrap_token: _,
            pubkey: _,
        } = binding
        else {
            return Err(MobError::Internal(
                "bind requested for non-external runtime binding".to_string(),
            ));
        };
        let authorized_peer =
            self.authorize_existing_peer_binding_for_rebind(binding, "bind_peer_only_member")?;
        if authorized_peer.name != peer.name
            || authorized_peer.peer_id != peer.peer_id
            || authorized_peer.address != peer.address
            || authorized_peer.pubkey != peer.pubkey
        {
            return Err(MobError::WiringError(format!(
                "bind requested for peer '{peer_id}' without matching MobMachine member peer authority"
            )));
        }
        let bootstrap_token = Self::bridge_bootstrap_token_from_binding(binding)?;
        let command = super::bridge_protocol::BridgeCommand::BindMember(
            super::bridge_protocol::BridgeBindPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
                expected_peer_id: authorized_peer.peer_id.to_string(),
                expected_address: authorized_peer.address.to_string(),
                bootstrap_token,
            },
        );
        let bind: super::bridge_protocol::BridgeBindResponse = self
            .send_bridge_command_typed(
                &authorized_peer,
                &command,
                std::time::Duration::from_secs(30),
            )
            .await?;
        let returned_address = super::bridge_protocol::canonicalize_bridge_address(&bind.address);
        let authorized_peer_id = authorized_peer.peer_id.to_string();
        let authorized_address = authorized_peer.address.to_string();
        let expected_address =
            super::bridge_protocol::canonicalize_bridge_address(&authorized_address);
        if bind.peer_id != authorized_peer_id || returned_address != expected_address {
            return Err(MobError::WiringError(format!(
                "bind response changed authorized endpoint for peer '{peer_id}'"
            )));
        }
        Ok(AuthorizedPeerOnlyBind {
            peer: authorized_peer,
            response: bind,
        })
    }

    fn bridge_rejection_reply(
        protocol_version: super::bridge_protocol::BridgeProtocolVersion,
        value: &serde_json::Value,
    ) -> Option<super::bridge_protocol::BridgeRejectionReply> {
        super::bridge_protocol::decode_bridge_rejection_reply(protocol_version, value)
    }

    fn bridge_rejection_error(rejection: super::bridge_protocol::BridgeRejectionReply) -> MobError {
        MobError::from(rejection)
    }

    fn bridge_rejection_error_with_reason(
        rejection: &super::bridge_protocol::BridgeRejectionReply,
        reason: String,
    ) -> MobError {
        match rejection.typed_cause() {
            Some(cause) => MobError::BridgeCommandRejected { cause, reason },
            None => MobError::WiringError(reason),
        }
    }

    async fn persist_rebound_binding(
        &mut self,
        prior_binding: &crate::RuntimeBinding,
        authorized_peer: &TrustedPeerDescriptor,
        bind_response: &super::bridge_protocol::BridgeBindResponse,
    ) -> Result<(), MobError> {
        let crate::RuntimeBinding::External {
            peer_id: prior_peer_id,
            address: prior_address,
            pubkey,
            ..
        } = prior_binding
        else {
            return Ok(());
        };
        let bootstrap_token = Some(Self::bridge_bootstrap_token_from_binding(prior_binding)?);
        let authorized_peer_id = authorized_peer.peer_id.to_string();
        let authorized_address = authorized_peer.address.to_string();
        let canonical_authorized_address =
            super::bridge_protocol::canonicalize_bridge_address(&authorized_address);
        let expected_address = super::bridge_protocol::canonicalize_bridge_address(prior_address);
        if authorized_peer_id != *prior_peer_id
            || canonical_authorized_address != expected_address
            || Some(authorized_peer.pubkey) != *pubkey
        {
            return Err(MobError::WiringError(format!(
                "rebound peer binding for '{prior_peer_id}' lacks matching generated MobMachine endpoint authority"
            )));
        }
        let returned_address =
            super::bridge_protocol::canonicalize_bridge_address(&bind_response.address);
        if bind_response.peer_id != authorized_peer_id
            || returned_address != canonical_authorized_address
        {
            return Err(MobError::WiringError(format!(
                "rebound peer binding for '{prior_peer_id}' attempted to change MobMachine-authorized endpoint"
            )));
        }

        let affected_identities: BTreeSet<_> = self
            .dsl_authority
            .state()
            .member_peer_ids
            .iter()
            .filter(|(_, peer_id)| peer_id.0 == authorized_peer_id)
            .map(|(identity, _)| identity.clone())
            .collect();
        if affected_identities.is_empty() {
            return Err(MobError::WiringError(format!(
                "rebound peer binding for '{prior_peer_id}' requires MobMachine member peer authority"
            )));
        }

        let affected_domain_identities: BTreeSet<_> = affected_identities
            .iter()
            .map(|identity| AgentIdentity::from(identity.0.as_str()))
            .collect();
        let updated_entries = self
            .roster
            .write()
            .await
            .replace_backend_peer_binding_for_identities(
                &affected_domain_identities,
                &authorized_peer_id,
                &canonical_authorized_address,
                bootstrap_token.clone(),
            );
        for (identity, generation, pubkey) in updated_entries {
            self.runtime_metadata
                .upsert_external_binding_overlay(
                    &self.definition.id,
                    &crate::store::ExternalBindingOverlayRecord {
                        agent_identity: identity,
                        generation,
                        normalized_member_ref: Some(MemberRef::BackendPeer {
                            peer_id: authorized_peer_id.clone(),
                            address: canonical_authorized_address.clone(),
                            pubkey,
                            bootstrap_token: None,
                            session_id: None,
                        }),
                        bootstrap_token: bootstrap_token.clone(),
                        status: crate::store::ExternalBindingOverlayStatus::Normalized,
                        updated_at: chrono::Utc::now(),
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn authorize_peer_only_member_ref_for_behavior(
        &mut self,
        member_ref: &MemberRef,
        context: &'static str,
    ) -> Result<MemberRef, MobError> {
        let Some(binding) = Self::runtime_binding_for_member_ref(member_ref) else {
            return Ok(member_ref.clone());
        };
        let peer = Self::peer_only_spec_for_binding(&binding, context)?;
        let peer = self
            .ensure_supervisor_authorized(&peer, Some(&binding))
            .await?;
        let bootstrap_token = Some(Self::bridge_bootstrap_token_from_binding(&binding)?);
        Ok(MemberRef::BackendPeer {
            peer_id: peer.peer_id.to_string(),
            address: peer.address.to_string(),
            pubkey: Some(peer.pubkey),
            bootstrap_token,
            session_id: None,
        })
    }

    async fn ensure_supervisor_authorized(
        &mut self,
        peer: &TrustedPeerDescriptor,
        binding: Option<&crate::RuntimeBinding>,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let payload = self.bridge_supervisor_payload().await?;
        let protocol_version = payload.protocol_version;
        let command = super::bridge_protocol::BridgeCommand::AuthorizeSupervisor(payload);
        self.supervisor_bridge.trust_recipient(peer).await?;
        let value = self
            .supervisor_bridge
            .send_bridge_command(peer, &command, std::time::Duration::from_secs(30))
            .await?;
        if let Some(rejection) = Self::bridge_rejection_reply(protocol_version, &value) {
            let should_rebind = match rejection.typed_cause() {
                Some(cause) => self.classify_bridge_rejection_recovery(cause)?,
                None => false,
            };
            if should_rebind && let Some(binding) = binding {
                let authorized_bind = self
                    .bind_peer_only_member_for_binding(peer, binding)
                    .await?;
                self.persist_rebound_binding(
                    binding,
                    &authorized_bind.peer,
                    &authorized_bind.response,
                )
                .await?;
                let prior_peer_id = match binding {
                    crate::RuntimeBinding::External { peer_id, .. } => peer_id.clone(),
                    crate::RuntimeBinding::Session => String::new(),
                };
                self.clear_pending_supervisor_acceptance_for_peer_ids(&[
                    prior_peer_id,
                    authorized_bind.peer.peer_id.to_string(),
                ])
                .await?;
                return Ok(authorized_bind.peer);
            }
            return Err(Self::bridge_rejection_error(rejection));
        }
        let _ack: super::bridge_protocol::BridgeAck =
            serde_json::from_value(value).map_err(|error| {
                MobError::Internal(format!(
                    "failed to decode authorize supervisor response: {error}"
                ))
            })?;
        Ok(peer.clone())
    }

    async fn load_supervisor_authority_snapshot(
        &self,
    ) -> Result<Option<SupervisorAuthorityLoad>, MobError> {
        let Some(durable) = self
            .runtime_metadata
            .load_supervisor_authority(&self.definition.id)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(SupervisorAuthorityLoad { durable }))
    }

    async fn load_supervisor_authority(
        &self,
    ) -> Result<Option<crate::store::SupervisorAuthorityRecord>, MobError> {
        Ok(self
            .load_supervisor_authority_snapshot()
            .await?
            .map(|loaded| loaded.durable))
    }

    async fn clear_pending_supervisor_acceptance_for_peer_ids(
        &mut self,
        peer_ids: &[String],
    ) -> Result<(), MobError> {
        if peer_ids.iter().all(|peer_id| peer_id.is_empty()) {
            return Ok(());
        }
        let Some(loaded) = self.load_supervisor_authority_snapshot().await? else {
            return Ok(());
        };
        let mut current = loaded.durable.clone();
        let Some(mut pending) = current.pending_rotation.clone() else {
            return Ok(());
        };
        if !pending.remove_accepted_peer_ids(peer_ids) {
            return Ok(());
        }
        current.pending_rotation = if pending.accepted_peer_ids.is_empty() {
            None
        } else {
            Some(pending)
        };
        let prepared = match current.pending_rotation.as_ref() {
            Some(pending) => self.prepare_supervisor_authority_persistence(
                current.dsl_record_pending_rotation_input(pending),
                &current,
                "clear_pending_supervisor_acceptance",
            )?,
            None => self.prepare_supervisor_authority_persistence(
                current.dsl_clear_pending_rotation_input(),
                &current,
                "clear_pending_supervisor_acceptance",
            )?,
        };
        match self
            .runtime_metadata
            .compare_and_put_supervisor_authority(
                &self.definition.id,
                &loaded.durable,
                &current,
                &prepared.authority,
            )
            .await
        {
            Ok(true) => {
                self.commit_prepared_dsl_transition(prepared.transition)?;
                Ok(())
            }
            Ok(false) => Err(MobError::from(crate::store::MobStoreError::CasConflict(
                format!(
                    "supervisor authority changed while clearing pending acceptance for mob '{}'",
                    self.definition.id
                ),
            ))),
            Err(error) => Err(MobError::from(error)),
        }
    }

    async fn send_bridge_command_typed<R: DeserializeOwned>(
        &self,
        peer: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: std::time::Duration,
    ) -> Result<R, MobError> {
        self.supervisor_bridge.trust_recipient(peer).await?;
        let value = self
            .supervisor_bridge
            .send_bridge_command(peer, command, timeout)
            .await?;
        if let Some(rejection) = Self::bridge_rejection_reply(command.protocol_version(), &value) {
            return Err(Self::bridge_rejection_error(rejection));
        }
        serde_json::from_value(value).map_err(|error| {
            MobError::Internal(format!("failed to decode bridge command response: {error}"))
        })
    }

    fn bridge_ack_from_value(
        protocol_version: super::bridge_protocol::BridgeProtocolVersion,
        value: serde_json::Value,
        context: &str,
    ) -> Result<(), MobError> {
        if let Some(rejection) = Self::bridge_rejection_reply(protocol_version, &value) {
            return Err(Self::bridge_rejection_error(rejection));
        }
        let _ack: super::bridge_protocol::BridgeAck =
            serde_json::from_value(value).map_err(|error| {
                MobError::Internal(format!("failed to decode {context} response: {error}"))
            })?;
        Ok(())
    }

    async fn observe_peer_only_binding(
        &mut self,
        binding: &crate::RuntimeBinding,
        timeout: std::time::Duration,
    ) -> Result<super::bridge_protocol::BridgeObservationResponse, MobError> {
        let peer = Self::peer_only_spec_for_binding(binding, "observe_peer_only_binding")?;
        let peer = self
            .ensure_supervisor_authorized(&peer, Some(binding))
            .await?;
        let payload = self.bridge_supervisor_payload().await?;
        let command = super::bridge_protocol::BridgeCommand::ObserveMember(payload);
        self.send_bridge_command_typed(&peer, &command, timeout)
            .await
    }

    async fn destroy_peer_only_binding(
        &mut self,
        binding: &crate::RuntimeBinding,
        timeout: std::time::Duration,
    ) -> Result<super::bridge_protocol::BridgeDestroyResponse, MobError> {
        let peer = Self::peer_only_spec_for_binding(binding, "destroy_peer_only_binding")?;
        let peer = self
            .ensure_supervisor_authorized(&peer, Some(binding))
            .await?;
        let payload = self.bridge_supervisor_payload().await?;
        let command = super::bridge_protocol::BridgeCommand::DestroyMember(payload);
        self.send_bridge_command_typed(&peer, &command, timeout)
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn revoke_supervisor_for_binding(
        &mut self,
        binding: &crate::RuntimeBinding,
        timeout: std::time::Duration,
    ) -> Result<(), MobError> {
        let peer = Self::peer_only_spec_for_binding(binding, "revoke_supervisor_for_binding")?;
        let peer = self
            .ensure_supervisor_authorized(&peer, Some(binding))
            .await?;
        let payload = self.bridge_supervisor_payload().await?;
        let command = super::bridge_protocol::BridgeCommand::RevokeSupervisor(payload);
        let _ack: super::bridge_protocol::BridgeAck = self
            .send_bridge_command_typed(&peer, &command, timeout)
            .await?;
        Ok(())
    }

    async fn wire_peer_only_recipient(
        &mut self,
        recipient: &TrustedPeerDescriptor,
        recipient_binding: Option<&crate::RuntimeBinding>,
        peer_spec: &TrustedPeerDescriptor,
        timeout: std::time::Duration,
    ) -> Result<(), MobError> {
        let recipient = self
            .ensure_supervisor_authorized(recipient, recipient_binding)
            .await?;
        let mob_peer_overlay =
            self.mob_peer_overlay_for_recipient(&recipient, "wire_peer_only_recipient")?;
        if !mob_peer_overlay
            .peers()
            .iter()
            .any(|peer| peer.peer_id == peer_spec.peer_id)
        {
            return Err(MobError::WiringError(format!(
                "wire_peer_only_recipient: peer '{}' is absent from MobMachine overlay for recipient '{}'",
                peer_spec.peer_id, recipient.peer_id
            )));
        }
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self.supervisor_bridge.supervisor_spec().await?;
        let command = super::bridge_protocol::BridgeCommand::WireMember(
            super::bridge_protocol::BridgePeerWiringPayload {
                supervisor: sup_spec.into(),
                epoch: authority.epoch,
                protocol_version: authority.protocol_version,
                peer_spec: peer_spec.clone().into(),
                mob_peer_overlay: mob_peer_overlay.bridge_handoff(),
            },
        );
        let _ack: super::bridge_protocol::BridgeAck = self
            .send_bridge_command_typed(&recipient, &command, timeout)
            .await?;
        Ok(())
    }

    async fn unwire_peer_only_recipient(
        &mut self,
        recipient: &TrustedPeerDescriptor,
        recipient_binding: Option<&crate::RuntimeBinding>,
        peer_spec: &TrustedPeerDescriptor,
        timeout: std::time::Duration,
    ) -> Result<(), MobError> {
        let recipient = self
            .ensure_supervisor_authorized(recipient, recipient_binding)
            .await?;
        let mob_peer_overlay =
            self.mob_peer_overlay_for_recipient(&recipient, "unwire_peer_only_recipient")?;
        if mob_peer_overlay
            .peers()
            .iter()
            .any(|peer| peer.peer_id == peer_spec.peer_id)
        {
            return Err(MobError::WiringError(format!(
                "unwire_peer_only_recipient: peer '{}' is still present in MobMachine overlay for recipient '{}'",
                peer_spec.peer_id, recipient.peer_id
            )));
        }
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self.supervisor_bridge.supervisor_spec().await?;
        let command = super::bridge_protocol::BridgeCommand::UnwireMember(
            super::bridge_protocol::BridgePeerWiringPayload {
                supervisor: sup_spec.into(),
                epoch: authority.epoch,
                protocol_version: authority.protocol_version,
                peer_spec: peer_spec.clone().into(),
                mob_peer_overlay: mob_peer_overlay.bridge_handoff(),
            },
        );
        let _ack: super::bridge_protocol::BridgeAck = self
            .send_bridge_command_typed(&recipient, &command, timeout)
            .await?;
        Ok(())
    }

    fn mob_peer_overlay_for_recipient(
        &mut self,
        recipient: &TrustedPeerDescriptor,
        context: &'static str,
    ) -> Result<super::provisioner::PeerOnlyTrustOverlay, MobError> {
        let state = self.dsl_authority.state();
        let expected_endpoint = mob_dsl::MemberPeerEndpoint::from(recipient);
        let mut recipient_identity = None;
        for (identity, endpoint) in &state.member_peer_endpoints {
            if *endpoint == expected_endpoint {
                if recipient_identity.replace(identity.clone()).is_some() {
                    return Err(MobError::WiringError(format!(
                        "{context}: recipient peer '{}' matches multiple MobMachine member endpoints",
                        recipient.peer_id
                    )));
                }
            }
        }
        let recipient_identity = recipient_identity.ok_or_else(|| {
            MobError::WiringError(format!(
                "{context}: recipient peer '{}' is outside MobMachine member endpoint authority",
                recipient.peer_id
            ))
        })?;
        let recipient_peer_id = recipient.peer_id.to_string();
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::AuthorizeMemberPeerOverlay {
                agent_identity: recipient_identity.clone(),
                expected_peer_endpoint: expected_endpoint,
            },
            context,
        )?;
        let obligation =
            crate::generated::protocol_mob_member_peer_overlay::extract_obligations_with_freshness(
                &transition,
                crate::generated::protocol_mob_member_peer_overlay::MobTopologyFreshnessAuthority::from_live_topology_epoch(
                    self.dsl_topology_epoch.clone(),
                    Arc::clone(&self.dsl_authority_owner_token),
                ),
            )
            .into_iter()
            .find(|obligation| {
                obligation.agent_identity() == &recipient_identity
                    && obligation.peer_id().0.as_str() == recipient_peer_id.as_str()
            })
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "{context}: generated MobMachine peer overlay handoff missing for recipient '{}'",
                    recipient.peer_id
                ))
            })?;
        super::provisioner::PeerOnlyTrustOverlay::from_generated_mob_member_peer_overlay(
            &obligation,
        )
    }

    /// Mirror MobMachine's terminality verdict for an observed remote-member
    /// runtime state.
    ///
    /// The bridge consumer extracts the pure wire runtime-state observation;
    /// MobMachine — not this shell — decides whether the observed state is
    /// terminal. We feed the raw observation and mirror the emitted verdict,
    /// failing closed (treating the observation as non-terminal, which forces a
    /// conservative destroy) only if the machine emits no verdict.
    fn observation_is_terminal(
        &mut self,
        observation: &super::bridge_protocol::BridgeObservationResponse,
    ) -> Result<bool, MobError> {
        let observed_state = remote_member_runtime_observed_state(observation.state)?;
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::ClassifyRemoteMemberRuntimeObservation { observed_state },
            "classify_remote_member_runtime_observation",
        )?;
        let (effect_observed_state, terminality) = effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::RemoteMemberRuntimeTerminalityClassified {
                    observed_state,
                    terminality,
                } => Some((observed_state, terminality)),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted remote-member runtime observation but emitted no terminality verdict"
                        .into(),
                )
            })?;
        if effect_observed_state != observed_state {
            return Err(MobError::Internal(format!(
                "MobMachine remote-member terminality drift: input={observed_state:?}, effect={effect_observed_state:?}"
            )));
        }
        Ok(matches!(
            terminality,
            mob_dsl::MobRemoteMemberRuntimeTerminality::Terminal
        ))
    }

    /// Mirror MobMachine's bridge-rejection recovery verdict for a typed wire
    /// rejection cause.
    ///
    /// When an `AuthorizeSupervisor` command is rejected, the bridge consumer
    /// extracts the pure wire rejection cause; MobMachine — not this shell —
    /// owns whether that cause is recoverable by re-running `BindMember`
    /// (`RebindRecover`) or must bubble up as fatal (`FatalBubbleUp`). We feed
    /// the mapped cause and mirror the emitted verdict, returning `true` only
    /// for `RebindRecover`. Fails closed (returns an error) if the machine emits
    /// no verdict.
    fn classify_bridge_rejection_recovery(
        &mut self,
        cause: super::bridge_protocol::BridgeRejectionCause,
    ) -> Result<bool, MobError> {
        let rejection_cause = mob_bridge_rejection_cause(cause);
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::ClassifyBridgeRejectionRecovery { rejection_cause },
            "classify_bridge_rejection_recovery",
        )?;
        let (effect_cause, recovery) = effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::BridgeRejectionRecoveryClassified {
                    rejection_cause,
                    recovery,
                } => Some((rejection_cause, recovery)),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted bridge rejection cause but emitted no recovery verdict"
                        .into(),
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

    /// Mirror MobMachine's pending-supervisor-acceptance verdict for a typed
    /// wire rejection cause.
    ///
    /// When an already-accepted remote peer is re-verified during supervisor
    /// rotation (the `AuthorizeSupervisor` command is replayed under the pending
    /// authority) and the member replies with a rejection, the actor extracts
    /// the pure wire rejection cause; MobMachine — not this shell — owns whether
    /// that cause means the prior acceptance is NOT confirmed and the peer must
    /// be re-attempted (`NotConfirmedReattempt`), the pending authority is stale
    /// (`StalePendingAuthority`), or the rejection is hard fatal (`Fatal`). The
    /// classification is stateless, so we drive it on a prepared authority
    /// (discarded) to keep this an immutable observation. Fails closed (returns
    /// `Fatal`'s caller-side error) if the machine emits no verdict.
    fn classify_pending_supervisor_acceptance(
        &self,
        cause: super::bridge_protocol::BridgeRejectionCause,
    ) -> Result<mob_dsl::MobPendingSupervisorAcceptanceKind, MobError> {
        let rejection_cause = mob_bridge_rejection_cause(cause);
        let prepared = self.prepare_dsl_input_transition(
            mob_dsl::MobMachineInput::ClassifyPendingSupervisorAcceptance { rejection_cause },
            "classify_pending_supervisor_acceptance",
        )?;
        let (effect_cause, verdict) = prepared
            .transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::PendingSupervisorAcceptanceClassified {
                    rejection_cause,
                    verdict,
                } => Some((*rejection_cause, *verdict)),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted pending supervisor acceptance cause but emitted no verdict"
                        .into(),
                )
            })?;
        if effect_cause != rejection_cause {
            return Err(MobError::Internal(format!(
                "MobMachine pending-supervisor-acceptance drift: input={rejection_cause:?}, effect={effect_cause:?}"
            )));
        }
        Ok(verdict)
    }

    /// Issue a strictly increasing fence token.
    fn issue_fence_token(&self) -> crate::ids::FenceToken {
        let val = self
            .next_fence_token
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::ids::FenceToken::new(val)
    }

    fn next_fence_token_preview(&self) -> crate::ids::FenceToken {
        crate::ids::FenceToken::new(
            self.next_fence_token
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    fn invalid_transition_to(&self, target: MobState) -> MobError {
        MobError::InvalidTransition {
            from: self.state(),
            to: target,
        }
    }

    async fn restore_failure_for(
        &self,
        agent_identity: &MeerkatId,
    ) -> Option<super::handle::RestoreFailureDiagnostic> {
        self.restore_diagnostics
            .read()
            .await
            .get(agent_identity)
            .cloned()
    }

    async fn ensure_member_not_broken(&self, agent_identity: &MeerkatId) -> Result<(), MobError> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&crate::ids::AgentIdentity::from(
            agent_identity.as_str(),
        ));
        let lifecycle = self
            .dsl_authority
            .state()
            .member_lifecycle_for_identity(&dsl_identity);
        if lifecycle.status == mob_dsl::MobMemberLifecycleStatus::Broken {
            let diag = self.restore_failure_for(agent_identity).await.unwrap_or(
                super::handle::RestoreFailureDiagnostic {
                    bridge_session_id: None,
                    reason: lifecycle
                        .error
                        .unwrap_or_else(|| "member restore failed".to_string()),
                },
            );
            return Err(MobError::MemberRestoreFailed {
                member_id: agent_identity.clone(),
                session_id: diag.bridge_session_id,
                reason: diag.reason,
            });
        }
        Ok(())
    }

    fn dsl_state(&self) -> MobState {
        project_dsl_phase(self.dsl_authority.state().lifecycle_phase)
    }

    fn destroy_admitted(&self) -> bool {
        self.dsl_authority.state().destroy_admitted
    }

    /// Project observable shell phase. A durable `MobDestroying` event closes
    /// public live authority before all cleanup necessarily completes, so the
    /// same-process projection must match restart projection and fail closed.
    fn state(&self) -> MobState {
        if self.destroy_admitted() {
            MobState::Destroyed
        } else {
            self.dsl_state()
        }
    }

    fn publish_machine_state_projection(&self) {
        self.dsl_topology_epoch.store(
            self.dsl_authority.state().topology_epoch,
            std::sync::atomic::Ordering::Release,
        );
        let _ = self
            .machine_state_watch_tx
            .send(self.dsl_authority.state().clone());
    }

    fn apply_dsl_input(
        &mut self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<(), MobError> {
        self.apply_dsl_input_collect_effects(input, context)
            .map(|_| ())
    }

    fn apply_dsl_input_collect_effects(
        &mut self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<Vec<mob_dsl::MobMachineEffect>, MobError> {
        Ok(self
            .apply_dsl_input_collect_transition(input, context)?
            .into_effects())
    }

    fn apply_dsl_input_collect_transition(
        &mut self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<mob_dsl::MobMachineTransition, MobError> {
        let input_debug = format!("{input:?}");
        let transition = mob_dsl::MobMachineMutator::apply(&mut self.dsl_authority, input)
            .map_err(|e| {
                MobError::Internal(format!(
                    "DSL authority ({context}) rejected {input_debug}: {e}"
                ))
            })?;
        self.queue_routed_effects_from(transition.effects());
        if transition.from_phase != transition.to_phase {
            // Publish the projected phase for external observers. This is
            // the sole write seam for the dogma-#13 projection watch.
            let _ = self.phase_watch_tx.send(self.state());
        }
        self.publish_machine_state_projection();
        Ok(transition)
    }

    fn prepare_dsl_input(
        &self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<PreparedDslInput, MobError> {
        self.prepare_dsl_inputs(std::slice::from_ref(&input), context)
    }

    fn prepare_dsl_input_transition(
        &self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<PreparedDslTransition, MobError> {
        let input_debug = format!("{input:?}");
        let mut authority = self.dsl_authority.prepare_authority();
        let transition = mob_dsl::MobMachineMutator::apply(&mut authority, input).map_err(|e| {
            MobError::Internal(format!(
                "DSL authority prepare ({context}) rejected {input_debug}: {e}"
            ))
        })?;
        Ok(PreparedDslTransition {
            authority,
            transition,
        })
    }

    fn prepare_dsl_signal_transition(
        &self,
        signal: mob_dsl::MobMachineSignal,
        context: &str,
    ) -> Result<PreparedDslTransition, MobError> {
        let signal_debug = format!("{signal:?}");
        let mut authority = self.dsl_authority.prepare_authority();
        let transition = authority.apply_signal(signal).map_err(|e| {
            MobError::Internal(format!(
                "DSL authority prepare ({context}): {e}; signal={signal_debug}; live_runtime_ids={:?}; runtime_fence_tokens={:?}",
                self.dsl_authority.state().live_runtime_ids,
                self.dsl_authority.state().runtime_fence_tokens,
            ))
        })?;
        Ok(PreparedDslTransition {
            authority,
            transition,
        })
    }

    fn prepare_dsl_inputs(
        &self,
        inputs: &[mob_dsl::MobMachineInput],
        context: &str,
    ) -> Result<PreparedDslInput, MobError> {
        let mut authority = self.dsl_authority.prepare_authority();
        let mut effects = Vec::new();
        let mut phase_changed = false;
        for input in inputs {
            let transition = mob_dsl::MobMachineMutator::apply(&mut authority, input.clone())
                .map_err(|e| {
                    let input_debug = format!("{input:?}");
                    MobError::Internal(format!(
                        "DSL authority prepare ({context}) rejected {input_debug}: {e}"
                    ))
                })?;
            if transition.from_phase != transition.to_phase {
                phase_changed = true;
            }
            effects.extend(transition.into_effects());
        }
        Ok(PreparedDslInput {
            authority,
            effects,
            phase_changed,
        })
    }

    fn commit_prepared_dsl_input(&mut self, prepared: PreparedDslInput) -> Result<(), MobError> {
        let effects = prepared.effects;
        let phase_changed = prepared.phase_changed;
        self.dsl_authority
            .commit_prepared_authority(prepared.authority)
            .map_err(|error| {
                MobError::Internal(format!("DSL authority prepared commit rejected: {error}"))
            })?;
        self.queue_routed_effects_from(&effects);
        if phase_changed {
            let _ = self.phase_watch_tx.send(self.state());
        }
        self.publish_machine_state_projection();
        Ok(())
    }

    async fn commit_prepared_dsl_input_after<T, F, Fut>(
        &mut self,
        prepared: PreparedDslInput,
        effect: F,
    ) -> Result<T, MobError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, MobError>>,
    {
        let effects = prepared.effects;
        let phase_changed = prepared.phase_changed;
        let result = self
            .dsl_authority
            .commit_prepared_authority_after(prepared.authority, effect)
            .await
            .map_err(|error| match error {
                mob_dsl::MobMachinePreparedCommitEffectError::Commit(error) => {
                    MobError::Internal(format!("DSL authority prepared commit rejected: {error}"))
                }
                mob_dsl::MobMachinePreparedCommitEffectError::Effect(error) => error,
            })?;
        self.queue_routed_effects_from(&effects);
        if phase_changed {
            let _ = self.phase_watch_tx.send(self.state());
        }
        self.publish_machine_state_projection();
        Ok(result)
    }

    fn commit_prepared_dsl_transition(
        &mut self,
        prepared: PreparedDslTransition,
    ) -> Result<(), MobError> {
        let phase_changed = prepared.transition.from_phase != prepared.transition.to_phase;
        let effects = prepared.transition.effects().to_vec();
        self.dsl_authority
            .commit_prepared_authority(prepared.authority)
            .map_err(|error| {
                MobError::Internal(format!("DSL authority prepared commit rejected: {error}"))
            })?;
        self.queue_routed_effects_from(&effects);
        if phase_changed {
            let _ = self.phase_watch_tx.send(self.state());
        }
        self.publish_machine_state_projection();
        Ok(())
    }

    async fn commit_prepared_dsl_transition_after<T, F, Fut>(
        &mut self,
        prepared: PreparedDslTransition,
        effect: F,
    ) -> Result<T, MobError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, MobError>>,
    {
        let phase_changed = prepared.transition.from_phase != prepared.transition.to_phase;
        let effects = prepared.transition.effects().to_vec();
        let result = self
            .dsl_authority
            .commit_prepared_authority_after(prepared.authority, effect)
            .await
            .map_err(|error| match error {
                mob_dsl::MobMachinePreparedCommitEffectError::Commit(error) => {
                    MobError::Internal(format!("DSL authority prepared commit rejected: {error}"))
                }
                mob_dsl::MobMachinePreparedCommitEffectError::Effect(error) => error,
            })?;
        self.queue_routed_effects_from(&effects);
        if phase_changed {
            let _ = self.phase_watch_tx.send(self.state());
        }
        self.publish_machine_state_projection();
        Ok(result)
    }

    fn prepare_supervisor_authority_persistence(
        &self,
        input: mob_dsl::MobMachineInput,
        record: &crate::store::SupervisorAuthorityRecord,
        context: &str,
    ) -> Result<PreparedSupervisorAuthorityPersistence, MobError> {
        let prepared = self.prepare_dsl_input_transition(input, context)?;
        let authority = crate::store::SupervisorAuthorityPersistenceAuthority::from_transition(
            record,
            &prepared.transition,
        )?;
        Ok(PreparedSupervisorAuthorityPersistence {
            transition: prepared,
            authority,
        })
    }

    fn prepare_supervisor_authority_deletion(
        &self,
        record: &crate::store::SupervisorAuthorityRecord,
        context: &str,
    ) -> Result<PreparedSupervisorAuthorityDeletion, MobError> {
        let prepared = self.prepare_dsl_input_transition(
            record.dsl_clear_authority_for_destroy_input(),
            context,
        )?;
        let authority = crate::store::SupervisorAuthorityDeletionAuthority::from_transition(
            record,
            &prepared.transition,
        )?;
        Ok(PreparedSupervisorAuthorityDeletion {
            transition: prepared,
            authority,
        })
    }

    fn supervisor_authority_record_is_machine_authorized(
        &self,
        record: &crate::store::SupervisorAuthorityRecord,
    ) -> bool {
        let state = self.dsl_authority.state();
        let peer_id = record.dsl_peer_id();
        let signing_key = record.dsl_signing_key();
        let protocol_version = record.dsl_protocol_version();
        let current_matches = state.supervisor_authority_peer_id.as_ref() == Some(&peer_id)
            && state.supervisor_authority_signing_key == Some(signing_key)
            && state.supervisor_authority_epoch == Some(record.epoch)
            && state.supervisor_authority_protocol_version == Some(protocol_version.clone());
        let pending_matches = state.supervisor_pending_authority_peer_id.as_ref() == Some(&peer_id)
            && state.supervisor_pending_authority_signing_key == Some(signing_key)
            && state.supervisor_pending_authority_epoch == Some(record.epoch)
            && state.supervisor_pending_authority_protocol_version == Some(protocol_version);
        current_matches || pending_matches
    }

    fn supervisor_bridge_authority_for_record(
        &self,
        record: &crate::store::SupervisorAuthorityRecord,
    ) -> Result<crate::store::SupervisorAuthorityBridgeAuthority, MobError> {
        Ok(
            crate::store::SupervisorAuthorityBridgeAuthority::from_machine_state(
                record,
                self.dsl_authority.state(),
            )?,
        )
    }

    fn require_lifecycle_journal_effect(
        transition: &mob_dsl::MobMachineTransition,
        kind: mob_dsl::MobLifecycleJournalKind,
        context: &str,
    ) -> Result<(), MobError> {
        if transition.effects().iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::AppendLifecycleJournal {
                    kind: effect_kind,
                    agent_identity: None,
                    agent_runtime_id: None,
                    fence_token: None,
                    generation: None,
                    session_id: None,
                }
                    if *effect_kind == kind
            )
        }) {
            Ok(())
        } else {
            Err(MobError::Internal(format!(
                "MobMachine {context} produced no generated {kind:?} lifecycle journal authority"
            )))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn require_member_lifecycle_journal_effect(
        transition: &mob_dsl::MobMachineTransition,
        kind: mob_dsl::MobLifecycleJournalKind,
        agent_identity: &AgentIdentity,
        agent_runtime_id: &crate::ids::AgentRuntimeId,
        fence_token: Option<crate::ids::FenceToken>,
        generation: crate::ids::Generation,
        session_id: Option<mob_dsl::SessionId>,
        context: &str,
    ) -> Result<(), MobError> {
        let expected_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let expected_runtime_id = mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id);
        let expected_fence = fence_token.map(mob_dsl::FenceToken::from_domain);
        let expected_generation = mob_dsl::Generation::from_domain(generation);
        let expected_session = session_id;
        if transition.effects().iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::AppendLifecycleJournal {
                    kind: effect_kind,
                    agent_identity: Some(effect_identity),
                    agent_runtime_id: Some(effect_runtime_id),
                    fence_token: effect_fence,
                    generation: Some(effect_generation),
                    session_id: effect_session,
                } if *effect_kind == kind
                    && effect_identity == &expected_identity
                    && effect_runtime_id == &expected_runtime_id
                    && effect_fence == &expected_fence
                    && effect_generation == &expected_generation
                    && effect_session == &expected_session
            )
        }) {
            Ok(())
        } else {
            Err(MobError::Internal(format!(
                "MobMachine {context} produced no generated {kind:?} lifecycle journal authority for member '{agent_identity}'"
            )))
        }
    }

    fn operator_action_recorded_event_from_generated_effect(
        transition: &mob_dsl::MobMachineTransition,
        context: &str,
    ) -> Result<MobEventKind, MobError> {
        let mut authorized = transition
            .effects()
            .iter()
            .filter_map(|effect| match effect {
                mob_dsl::MobMachineEffect::AppendOperatorActionProvenance {
                    tool_name,
                    principal_token,
                    caller_provenance,
                    audit_invocation_id,
                } => Some(MobEventKind::OperatorActionRecorded {
                    tool_name: tool_name.clone(),
                    principal_token: principal_token.clone(),
                    caller_provenance: caller_provenance.clone(),
                    audit_invocation_id: audit_invocation_id.clone(),
                }),
                _ => None,
            });
        let Some(event) = authorized.next() else {
            return Err(MobError::Internal(format!(
                "MobMachine {context} produced no generated operator provenance journal authority"
            )));
        };
        if authorized.next().is_some() {
            return Err(MobError::Internal(format!(
                "MobMachine {context} produced multiple generated operator provenance journal authorities"
            )));
        }
        Ok(event)
    }

    fn prepare_command_admission(
        &self,
        input: mob_dsl::MobMachineInput,
        target: MobState,
        context: &str,
    ) -> Result<PreparedDslInput, MobError> {
        self.prepare_dsl_input(input, context).map_err(|error| {
            tracing::debug!(
                context,
                error = %error,
                "MobMachine command admission rejected input"
            );
            self.invalid_transition_to(target)
        })
    }

    fn probe_command_admission(
        &self,
        input: mob_dsl::MobMachineInput,
        target: MobState,
        context: &str,
    ) -> Result<(), MobError> {
        self.prepare_command_admission(input, target, context)
            .map(|_| ())
    }

    fn apply_command_admission(
        &mut self,
        input: mob_dsl::MobMachineInput,
        target: MobState,
        context: &str,
    ) -> Result<Vec<mob_dsl::MobMachineEffect>, MobError> {
        self.apply_dsl_input_collect_effects(input, context)
            .map_err(|error| {
                tracing::debug!(
                    context,
                    error = %error,
                    "MobMachine command admission rejected input"
                );
                self.invalid_transition_to(target)
            })
    }

    fn machine_member_peer_spec_for(
        &self,
        identity: &AgentIdentity,
        context: &'static str,
    ) -> Result<Option<TrustedPeerDescriptor>, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        self.dsl_authority
            .state()
            .member_peer_endpoints
            .get(&dsl_identity)
            .map(|endpoint| Self::peer_only_spec_from_member_endpoint(endpoint, context))
            .transpose()
    }

    fn roster_member_peer_spec_for(
        &self,
        entry: &RosterEntry,
        context: &'static str,
    ) -> Result<Option<TrustedPeerDescriptor>, MobError> {
        let Some(peer_id) = entry.peer_id else {
            return Ok(None);
        };
        let Some(public_key) = entry.transport_public_key.as_deref() else {
            return Ok(None);
        };
        let pubkey = meerkat_comms::PubKey::from_pubkey_string(public_key).map_err(|error| {
            MobError::WiringError(format!(
                "{context}: invalid retained peer key for '{}': {error}",
                entry.agent_identity
            ))
        })?;
        let comms_name = self.comms_name_for(entry);
        TrustedPeerDescriptor::unsigned_with_pubkey(
            comms_name.clone(),
            peer_id.to_string(),
            *pubkey.as_bytes(),
            format!("inproc://{comms_name}"),
        )
        .map(Some)
        .map_err(|error| {
            MobError::WiringError(format!(
                "{context}: invalid retained peer descriptor for '{}': {error}",
                entry.agent_identity
            ))
        })
    }

    async fn retained_member_peer_spec_from_wired_peer_trust(
        &self,
        entry: &RosterEntry,
        peer_identities: &BTreeSet<AgentIdentity>,
        context: &'static str,
    ) -> Result<Option<TrustedPeerDescriptor>, MobError> {
        let expected_name = self.comms_name_for(entry);
        let mut retained = None;
        for peer_identity in peer_identities {
            let peer_entry = {
                let roster = self.roster.read().await;
                roster.get_by_identity(peer_identity).cloned()
            };
            let Some(peer_entry) = peer_entry else {
                continue;
            };
            let Some(comms) = self.provisioner_comms(&peer_entry.member_ref).await else {
                continue;
            };
            let trusted_peers = comms
                .trusted_peer_projection_snapshot_for_source(
                    meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
                )
                .await
                .map_err(|error| {
                    MobError::WiringError(format!(
                        "{context}: failed to read retained trusted peers for '{}': {error}",
                        peer_entry.agent_identity
                    ))
                })?;
            for peer in trusted_peers {
                if peer.name.as_str() != expected_name {
                    continue;
                }
                if let Some(previous) = retained.replace(peer.clone())
                    && previous.peer_id != peer.peer_id
                {
                    return Err(MobError::WiringError(format!(
                        "{context}: retained peer descriptor for '{}' disagrees across wired peers",
                        entry.agent_identity
                    )));
                }
            }
        }
        Ok(retained)
    }

    fn effects_include_wiring_graph_change(effects: &[mob_dsl::MobMachineEffect]) -> bool {
        effects
            .iter()
            .any(|effect| matches!(effect, mob_dsl::MobMachineEffect::WiringGraphChanged { .. }))
    }

    fn wire_members_disposition_from_effects(
        effects: &[mob_dsl::MobMachineEffect],
        edge: &mob_dsl::WiringEdge,
        context: &str,
    ) -> Result<bool, MobError> {
        let graph_changed = Self::effects_include_wiring_graph_change(effects);
        let repair_requested = effects.iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::WiringTrustRepairRequested {
                    edge: effect_edge
                } if effect_edge == edge
            )
        });
        match (graph_changed, repair_requested) {
            (true, false) => Ok(true),
            (false, true) => Ok(false),
            (false, false) => Err(MobError::WiringError(format!(
                "{context} produced no generated wiring graph or repair authority for edge {edge:?}"
            ))),
            (true, true) => Err(MobError::WiringError(format!(
                "{context} produced conflicting generated wiring graph and repair authority for edge {edge:?}"
            ))),
        }
    }

    fn member_trust_wiring_handoff_from_transition(
        &self,
        transition: &mob_dsl::MobMachineTransition,
        edge: &mob_dsl::WiringEdge,
        context: &str,
        operation: MemberTrustOperation,
    ) -> Result<MemberTrustHandoff, MobError> {
        self.member_trust_wiring_handoff_from_transition_with_freshness(
            transition,
            edge,
            crate::generated::protocol_mob_member_trust_wiring::MobTopologyFreshnessAuthority::from_live_member_trust_authority(
                &self.dsl_authority,
            ),
            context,
            operation,
        )
    }

    fn member_trust_wiring_handoff_from_prepared_batch_transition(
        &self,
        transition: &mob_dsl::MobMachineTransition,
        edge: &mob_dsl::WiringEdge,
        freshness_authority: crate::generated::protocol_mob_member_trust_wiring::MobTopologyFreshnessAuthority,
        context: &str,
        operation: MemberTrustOperation,
    ) -> Result<MemberTrustHandoff, MobError> {
        self.member_trust_wiring_handoff_from_transition_with_freshness(
            transition,
            edge,
            freshness_authority,
            context,
            operation,
        )
    }

    fn member_trust_wiring_handoff_from_transition_with_freshness(
        &self,
        transition: &mob_dsl::MobMachineTransition,
        edge: &mob_dsl::WiringEdge,
        mob_topology_freshness_authority: crate::generated::protocol_mob_member_trust_wiring::MobTopologyFreshnessAuthority,
        context: &str,
        operation: MemberTrustOperation,
    ) -> Result<MemberTrustHandoff, MobError> {
        let obligation =
            crate::generated::protocol_mob_member_trust_wiring::extract_obligations_with_freshness(
                transition,
                mob_topology_freshness_authority,
            )
            .into_iter()
            .find(|obligation| obligation.edge() == edge)
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "{context} produced no generated member wiring trust obligation"
                ))
            })?;
        let authority = match operation {
            MemberTrustOperation::Wiring => MemberTrustAuthority::Wiring(obligation),
            MemberTrustOperation::Repair => MemberTrustAuthority::Repair(obligation),
            MemberTrustOperation::Unwiring => {
                return Err(MobError::WiringError(
                    "member unwiring cannot use a wiring trust obligation".to_string(),
                ));
            }
        };
        Ok(MemberTrustHandoff {
            edge: edge.clone(),
            authority,
            operation,
        })
    }

    fn wire_external_authority_from_transition(
        &self,
        transition: &mob_dsl::MobMachineTransition,
        edge: &mob_dsl::ExternalPeerEdge,
        context: &str,
    ) -> Result<WireTrustAuthority, MobError> {
        let effects = transition.effects();
        let graph_changed = Self::effects_include_wiring_graph_change(effects);
        let wiring_obligation =
            crate::generated::protocol_mob_external_peer_trust_wiring::extract_obligations_with_freshness(
                transition,
                crate::generated::protocol_mob_external_peer_trust_wiring::MobTopologyFreshnessAuthority::from_live_topology_epoch(self.dsl_topology_epoch.clone(), Arc::clone(&self.dsl_authority_owner_token)),
            )
            .into_iter()
            .find(|obligation| obligation.edge() == edge);
        let repair_obligation =
            crate::generated::protocol_mob_external_peer_trust_repair::extract_obligations_with_freshness(
                transition,
                crate::generated::protocol_mob_external_peer_trust_repair::MobTopologyFreshnessAuthority::from_live_topology_epoch(self.dsl_topology_epoch.clone(), Arc::clone(&self.dsl_authority_owner_token)),
            )
            .into_iter()
            .find(|obligation| obligation.edge() == edge);
        let repair_requested = repair_obligation.is_some();
        match (graph_changed, repair_requested) {
            (true, false) => {
                let obligation = wiring_obligation.ok_or_else(|| {
                    MobError::WiringError(format!(
                        "{context} produced external graph change without generated wiring trust obligation"
                    ))
                })?;
                let expected_peer_id = edge.endpoint.peer_id.0.as_str();
                Ok(WireTrustAuthority::ExternalGraphAdded(
                    crate::generated::protocol_mob_external_peer_trust_wiring::wiring_authority_for_peer(
                        &obligation,
                        expected_peer_id,
                    )
                    .map_err(MobError::WiringError)?,
                ))
            }
            (false, true) => {
                let obligation = repair_obligation.ok_or_else(|| {
                    MobError::WiringError(format!(
                        "{context} produced external repair marker without generated repair trust obligation"
                    ))
                })?;
                let expected_peer_id = edge.endpoint.peer_id.0.as_str();
                Ok(WireTrustAuthority::ExternalRepairRequested(
                    crate::generated::protocol_mob_external_peer_trust_repair::repair_authority_for_peer(
                        &obligation,
                        expected_peer_id,
                    )
                    .map_err(MobError::WiringError)?,
                ))
            }
            (false, false) => Err(MobError::WiringError(format!(
                "{context} produced no generated external-peer trust authority"
            ))),
            (true, true) => Err(MobError::WiringError(format!(
                "{context} produced conflicting generated external-peer trust authority"
            ))),
        }
    }

    fn unwire_members_authority_from_transition(
        &self,
        transition: &mob_dsl::MobMachineTransition,
        edge: &mob_dsl::WiringEdge,
        context: &str,
    ) -> Result<MemberTrustHandoff, MobError> {
        let obligation = crate::generated::protocol_mob_member_trust_unwiring::extract_obligations_with_freshness(
            transition,
            crate::generated::protocol_mob_member_trust_unwiring::MobTopologyFreshnessAuthority::from_live_topology_epoch(self.dsl_topology_epoch.clone(), Arc::clone(&self.dsl_authority_owner_token)),
        )
        .into_iter()
        .find(|obligation| obligation.edge() == edge)
        .ok_or_else(|| {
            MobError::WiringError(format!(
                "{context} produced no generated member unwiring trust obligation"
            ))
        })?;
        Ok(MemberTrustHandoff {
            edge: edge.clone(),
            authority: MemberTrustAuthority::Unwiring(obligation),
            operation: MemberTrustOperation::Unwiring,
        })
    }

    fn authorize_member_trust_wiring(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        context: &str,
        operation: MemberTrustOperation,
    ) -> Result<MemberTrustHandoff, MobError> {
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::AuthorizeMemberTrustWiring {
                edge: edge.clone(),
                a_identity: edge.a.clone(),
                b_identity: edge.b.clone(),
            },
            context,
        )?;
        self.member_trust_wiring_handoff_from_transition(&transition, edge, context, operation)
    }

    fn authorize_member_trust_unwiring(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        context: &str,
    ) -> Result<MemberTrustHandoff, MobError> {
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::AuthorizeMemberTrustUnwiring {
                edge: edge.clone(),
                a_identity: edge.a.clone(),
                b_identity: edge.b.clone(),
            },
            context,
        )?;
        self.unwire_members_authority_from_transition(&transition, edge, context)
    }

    fn authorize_member_trust_cleanup(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        context: &str,
    ) -> Result<MemberTrustHandoff, MobError> {
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::AuthorizeMemberTrustCleanup {
                edge: edge.clone(),
                a_identity: edge.a.clone(),
                b_identity: edge.b.clone(),
            },
            context,
        )?;
        self.unwire_members_authority_from_transition(&transition, edge, context)
    }

    fn authorize_member_trust_cleanup_observed(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        a_identity: &AgentIdentity,
        a_peer_id: &str,
        b_identity: &AgentIdentity,
        b_peer_id: &str,
        context: &str,
    ) -> Result<MemberTrustHandoff, MobError> {
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::AuthorizeMemberTrustCleanupObserved {
                edge: edge.clone(),
                a_identity: mob_dsl::AgentIdentity::from_domain(a_identity),
                a_peer_id: mob_dsl::PeerId(a_peer_id.to_string()),
                b_identity: mob_dsl::AgentIdentity::from_domain(b_identity),
                b_peer_id: mob_dsl::PeerId(b_peer_id.to_string()),
            },
            context,
        )?;
        self.unwire_members_authority_from_transition(&transition, edge, context)
    }

    fn preview_dsl_input(
        &self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<mob_dsl::MobMachineState, MobError> {
        let input_debug = format!("{input:?}");
        let mut authority =
            mob_dsl::MobMachineAuthority::recover_from_state(self.dsl_authority.state().clone())
                .map_err(|error| {
                    MobError::Internal(format!(
                        "DSL authority preview ({context}) could not recover state: {error}"
                    ))
                })?;
        let transition = mob_dsl::MobMachineMutator::apply(&mut authority, input).map_err(|e| {
            MobError::Internal(format!(
                "DSL authority preview ({context}) rejected {input_debug}: {e}"
            ))
        })?;
        let _ = transition;
        Ok(authority.state().clone())
    }

    fn apply_dsl_signal(
        &mut self,
        signal: mob_dsl::MobMachineSignal,
        context: &str,
    ) -> Result<(), MobError> {
        self.apply_dsl_signal_collect_transition(signal, context)
            .map(|_| ())
    }

    fn apply_dsl_signal_collect_transition(
        &mut self,
        signal: mob_dsl::MobMachineSignal,
        context: &str,
    ) -> Result<mob_dsl::MobMachineTransition, MobError> {
        let signal_debug = format!("{signal:?}");
        let transition = self
            .dsl_authority
            .apply_signal(signal)
            .map_err(|e| {
                MobError::Internal(format!(
                    "DSL authority ({context}): {e}; signal={signal_debug}; live_runtime_ids={:?}; runtime_fence_tokens={:?}",
                    self.dsl_authority.state().live_runtime_ids,
                    self.dsl_authority.state().runtime_fence_tokens,
                ))
        })?;
        self.queue_routed_effects_from(transition.effects());
        if transition.from_phase != transition.to_phase {
            let _ = self.phase_watch_tx.send(self.state());
        }
        self.publish_machine_state_projection();
        Ok(transition)
    }

    /// Wave-c C-6p — harvest routed seam effects from a DSL transition's
    /// effect list into the actor's pending-dispatch queue.
    ///
    /// Non-routed variants (persist-kickoff, emit-lifecycle-notice,
    /// topology-signal, etc.) stay on the in-process effect-drain path
    /// reached from the individual command handlers and never enter the
    /// composition dispatcher. Routed variants flow through
    /// `flush_routed_effects` at the next async boundary.
    fn queue_routed_effects_from(&mut self, effects: &[mob_dsl::MobMachineEffect]) {
        for effect in effects {
            if let Some(seam_effect) = super::composition::lift_routed_effect(effect) {
                self.pending_routed_effects.push(seam_effect);
            }
        }
    }

    /// Drain the pending-routed-effect queue, dispatching each payload
    /// through the typed [`CompositionBinding`]. Returns the first typed
    /// [`MobError`] a dispatch yields; subsequent effects remain queued
    /// so the next flush sees them (FIFO).
    ///
    /// In [`CompositionBinding::Standalone`] mode (single-machine / test
    /// construction) this drains the queue without attempting any
    /// dispatch — the producer has no consumer-side surface to target by
    /// construction. In [`CompositionBinding::Wired`] mode,
    /// [`DispatchRefusal::UnwiredConsumer`] surfaces as
    /// [`MobError::WiringError`] per the wave-c spine's intermediate-state
    /// contract (C-6p landed, C-6c pending).
    pub(super) async fn flush_routed_effects(&mut self) -> Result<(), MobError> {
        use super::composition::dispatch_routed_effect;
        while let Some(effect) = self.pending_routed_effects.first().cloned() {
            match dispatch_routed_effect(&self.composition_binding, effect).await {
                Ok(_outcome) => {
                    // Drop the head now that dispatch succeeded (or was a
                    // standalone no-op); keep subsequent queue entries
                    // intact so a later failure preserves FIFO ordering.
                    self.pending_routed_effects.remove(0);
                }
                Err(error) => {
                    // Preserve the head + tail on failure so the operator
                    // can inspect them; caller decides to retry or abort.
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn discard_pending_routed_effects_for_session(&mut self, session_id: &SessionId) {
        let dsl_session_id = mob_dsl::SessionId::from_domain(session_id);
        self.pending_routed_effects.retain(|effect| {
            !matches!(
                effect,
                super::composition::MobSeamEffect::Mob(
                    mob_dsl::MobMachineEffect::RequestRuntimeBinding {
                        session_id,
                        ..
                    }
                    | mob_dsl::MobMachineEffect::RequestRuntimeRetire { session_id }
                    | mob_dsl::MobMachineEffect::RequestRuntimeDestroy { session_id },
                ) if session_id == &dsl_session_id
            )
        });
    }

    /// Snapshot the DSL's current `member_state_markers` as a set of
    /// runtime-id keys (in their stringified DSL form) currently marked
    /// `Retiring`. The stringified form matches
    /// `AgentRuntimeId::Display` (`"identity:generation"`), which is what
    /// `mob_dsl::AgentRuntimeId::from_domain` produces.
    fn retiring_runtime_ids_from_dsl(&self) -> std::collections::BTreeSet<String> {
        self.dsl_authority
            .state()
            .member_state_markers
            .iter()
            .filter_map(|(runtime_id, member_state)| match member_state {
                mob_dsl::MobMemberState::Retiring => Some(runtime_id.0.clone()),
                mob_dsl::MobMemberState::Active => None,
            })
            .collect()
    }

    fn pending_kickoff_member_ids_from_dsl(&self) -> std::collections::BTreeSet<String> {
        self.dsl_authority
            .state()
            .member_kickoff_pending
            .iter()
            .chain(self.dsl_authority.state().member_kickoff_starting.iter())
            .chain(
                self.dsl_authority
                    .state()
                    .member_kickoff_callback_pending
                    .iter(),
            )
            .cloned()
            .collect()
    }

    fn ready_runtime_ids_from_dsl(&self) -> std::collections::BTreeSet<String> {
        self.dsl_authority
            .state()
            .member_startup_runtime_ready
            .iter()
            .chain(self.dsl_authority.state().member_startup_ready.iter())
            .map(|runtime_id| runtime_id.0.clone())
            .collect()
    }

    fn machine_projection_for_identity(
        &self,
        agent_identity: &crate::ids::AgentIdentity,
    ) -> super::state::MobMemberMachineProjection {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let dsl = self.dsl_authority.state();
        let runtime_id = dsl.identity_to_runtime.get(&dsl_identity).cloned();
        let state_marker = runtime_id
            .as_ref()
            .and_then(|runtime_id| dsl.member_state_markers.get(runtime_id).copied());
        let live_runtime = runtime_id
            .as_ref()
            .is_some_and(|runtime_id| dsl.live_runtime_ids.contains(runtime_id));
        let bound_session_id = dsl.member_session_bindings.get(&dsl_identity).cloned();
        super::state::MobMemberMachineProjection {
            runtime_id,
            state_marker,
            live_runtime,
            bound_session_id,
        }
    }

    fn machine_bridge_session_id_for_identity(
        &self,
        agent_identity: &crate::ids::AgentIdentity,
    ) -> Option<SessionId> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        self.dsl_authority
            .state()
            .member_session_bindings
            .get(&dsl_identity)
            .and_then(|session_id| SessionId::parse(&session_id.0).ok())
    }

    fn project_member_ref_session_binding(
        member_ref: &MemberRef,
        current_bridge_session_id: Option<SessionId>,
    ) -> Option<MemberRef> {
        match member_ref {
            MemberRef::Session { .. } => {
                current_bridge_session_id.map(MemberRef::from_bridge_session_id)
            }
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                ..
            } => Some(MemberRef::BackendPeer {
                peer_id: peer_id.clone(),
                address: address.clone(),
                pubkey: *pubkey,
                bootstrap_token: bootstrap_token.clone(),
                session_id: current_bridge_session_id,
            }),
        }
    }

    fn machine_member_ref_for_behavior(
        &self,
        entry: &RosterEntry,
        context: &str,
    ) -> Result<MemberRef, MobError> {
        let bridge_session_id = self.machine_bridge_session_id_for_identity(&entry.agent_identity);
        Self::project_member_ref_session_binding(&entry.member_ref, bridge_session_id).ok_or_else(
            || {
                MobError::Internal(format!(
                    "{context} requires MobMachine session binding for '{}'",
                    entry.agent_identity
                ))
            },
        )
    }

    async fn machine_member_material(
        &mut self,
        agent_identity: &MeerkatId,
        include_session_details: bool,
    ) -> Result<CanonicalMemberSnapshotMaterial, MobError> {
        let roster_entry = {
            let roster = self.roster.read().await;
            roster.get(agent_identity).cloned()
        };
        let domain_identity = crate::ids::AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let current_bridge_session_id =
            self.machine_bridge_session_id_for_identity(&domain_identity);
        let machine_runtime = self
            .dsl_authority
            .state()
            .member_runtime_material_for_identity(&dsl_identity)
            .map(|material| material.to_domain_for_identity(&domain_identity));
        let member_present = roster_entry.is_some();

        let (output_preview, tokens_used) = match current_bridge_session_id.as_ref() {
            None => (None, 0),
            Some(bridge_session_id) if include_session_details => {
                match self.session_service.read(bridge_session_id).await {
                    Ok(view) => (
                        view.state.last_assistant_text.clone(),
                        view.billing.total_tokens,
                    ),
                    Err(meerkat_core::service::SessionError::NotFound { .. }) => {
                        let _ = self
                            .record_missing_member_bridge_session(
                                agent_identity,
                                bridge_session_id,
                                "member_status",
                            )
                            .await;
                        (None, 0)
                    }
                    Err(_) => (None, 0),
                }
            }
            Some(_) => (None, 0),
        };
        let machine_lifecycle = self
            .dsl_authority
            .state()
            .member_lifecycle_for_identity(&dsl_identity);
        let kickoff = kickoff_snapshot_from_machine_state(
            agent_identity.as_str(),
            self.dsl_authority.state(),
            roster_entry
                .as_ref()
                .and_then(|entry| entry.kickoff.as_ref()),
        );

        Ok(MobMemberLifecycleProjection::materialize(
            MobMemberLifecycleInput {
                member_present,
                machine_lifecycle,
                output_preview,
                tokens_used,
                agent_identity: domain_identity,
                agent_runtime_id: machine_runtime
                    .as_ref()
                    .map(|(agent_runtime_id, _)| agent_runtime_id.clone()),
                fence_token: machine_runtime.map(|(_, fence_token)| fence_token),
                current_bridge_session_id,
                peer_connectivity: None,
                kickoff,
            },
        ))
    }

    async fn record_missing_member_bridge_session(
        &mut self,
        agent_identity: &MeerkatId,
        bridge_session_id: &SessionId,
        context: &'static str,
    ) -> Option<String> {
        let domain_identity = crate::ids::AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let current_binding_matches = self
            .dsl_authority
            .state()
            .member_session_bindings
            .get(&dsl_identity)
            .is_some_and(|session_id| session_id.0 == bridge_session_id.to_string());
        if !current_binding_matches {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                bridge_session_id = %bridge_session_id,
                context,
                "observed missing bridge session for a stale/non-current member binding"
            );
            return None;
        }
        if let Some(reason) = self
            .dsl_authority
            .state()
            .member_restore_failures
            .get(&dsl_identity)
            .cloned()
        {
            return Some(reason);
        }

        let reason = format!("missing bridge session snapshot for '{bridge_session_id}'");
        if let Err(error) = self.apply_dsl_signal(
            mob_dsl::MobMachineSignal::RecoverMemberRestoreFailure {
                agent_identity: dsl_identity,
                reason: reason.clone(),
            },
            "record_missing_member_bridge_session",
        ) {
            let fallback = format!("{reason}; failed to record restore failure: {error}");
            tracing::error!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                bridge_session_id = %bridge_session_id,
                context,
                error = %error,
                "MobMachine rejected member restore-failure observation"
            );
            return Some(fallback);
        }
        self.restore_diagnostics.write().await.insert(
            agent_identity.clone(),
            super::handle::RestoreFailureDiagnostic {
                bridge_session_id: Some(bridge_session_id.clone()),
                reason: reason.clone(),
            },
        );
        tracing::error!(
            mob_id = %self.definition.id,
            agent_identity = %agent_identity,
            bridge_session_id = %bridge_session_id,
            context,
            reason = %reason,
            "member bridge session is missing; marked member broken"
        );
        Some(reason)
    }

    fn active_machine_member_ids_for_profile(
        &self,
        profile_name: &ProfileName,
        excluded_identity: &MeerkatId,
    ) -> Vec<MeerkatId> {
        let dsl = self.dsl_authority.state();
        dsl.member_profile_names
            .iter()
            .filter_map(|(identity, machine_profile_name)| {
                if machine_profile_name.as_str() != profile_name.as_str()
                    || identity.0.as_str() == excluded_identity.as_str()
                    || !MobMemberLifecycleProjection::is_active_machine_lifecycle(
                        &dsl.member_lifecycle_for_identity(identity),
                    )
                {
                    return None;
                }
                Some(MeerkatId::from(identity.0.as_str()))
            })
            .collect()
    }

    async fn project_member_list_from_machine(
        &mut self,
        include_retiring: bool,
    ) -> Vec<MobMemberListEntry> {
        let entries_by_identity: BTreeMap<_, _> = {
            let roster = self.roster.read().await;
            roster
                .list_all()
                .cloned()
                .map(|entry| (entry.agent_identity.clone(), entry))
                .collect()
        };
        let machine_state = self.dsl_authority.state().clone();
        let mut projected = Vec::with_capacity(machine_state.identity_to_runtime.len());
        for identity in machine_state.identity_to_runtime.keys() {
            let Some(entry) =
                super::handle::MobHandle::project_member_list_entry_from_machine_identity(
                    identity,
                    entries_by_identity.get(&AgentIdentity::from(identity.0.as_str())),
                    &machine_state,
                )
            else {
                continue;
            };
            if !include_retiring && entry.state == crate::roster::MemberState::Retiring {
                continue;
            }
            projected.push(entry);
        }
        projected
    }

    fn mob_handle_for_tools(&self) -> MobHandle {
        MobHandle {
            command_tx: self.command_tx.clone(),
            roster: self.roster.clone(),
            definition: self.definition.clone(),
            events: self.events.clone(),
            run_store: self.run_store.clone(),
            flow_streams: self.flow_streams.clone(),
            session_service: self.session_service.clone(),
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: self.runtime_adapter.clone(),
            restore_diagnostics: self.restore_diagnostics.clone(),
            machine_state_watch_rx: self.machine_state_watch_tx.subscribe(),
            phase_watch_rx: self.phase_watch_tx.subscribe(),
            // W2-E: the actor's internal handle-for-tools does not carry the
            // realtime factory — that seam lives on the caller-facing
            // `MobHandle` returned from `MobBuilder`. Tools built from the
            // actor do not dial realtime endpoints.
            realtime_session_factory: None,
        }
    }

    async fn persist_kickoff_state(
        &self,
        agent_identity: &MeerkatId,
        phase: crate::roster::MobMemberKickoffPhase,
        error: Option<String>,
    ) -> Result<(), MobError> {
        let kickoff = crate::roster::MobMemberKickoffSnapshot {
            phase,
            error,
            updated_at: SystemTime::now(),
        };
        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberKickoffUpdated {
                    member: AgentIdentity::from(agent_identity.as_str()),
                    kickoff: kickoff.clone(),
                },
            })
            .await
            .map_err(MobError::from)?;
        self.roster
            .write()
            .await
            .set_kickoff(agent_identity, Some(kickoff));
        Ok(())
    }

    fn kickoff_phase_from_dsl(
        phase: mob_dsl::KickoffPhase,
    ) -> crate::roster::MobMemberKickoffPhase {
        match phase {
            mob_dsl::KickoffPhase::Pending => crate::roster::MobMemberKickoffPhase::Pending,
            mob_dsl::KickoffPhase::Starting => crate::roster::MobMemberKickoffPhase::Starting,
            mob_dsl::KickoffPhase::Started => crate::roster::MobMemberKickoffPhase::Started,
            mob_dsl::KickoffPhase::CallbackPending => {
                crate::roster::MobMemberKickoffPhase::CallbackPending
            }
            mob_dsl::KickoffPhase::Failed => crate::roster::MobMemberKickoffPhase::Failed,
            mob_dsl::KickoffPhase::Cancelled => crate::roster::MobMemberKickoffPhase::Cancelled,
        }
    }

    fn kickoff_notice_intent(
        intent: crate::machines::mob_machine::KickoffIntent,
    ) -> Option<&'static str> {
        use crate::machines::mob_machine::KickoffIntent;
        match intent {
            KickoffIntent::Failed => Some("mob.kickoff_failed"),
            KickoffIntent::Cancelled => Some("mob.kickoff_cancelled"),
            KickoffIntent::Pending
            | KickoffIntent::Starting
            | KickoffIntent::Started
            | KickoffIntent::CallbackPending => None,
        }
    }

    async fn clear_kickoff_state(&mut self, agent_identity: &MeerkatId) {
        match self
            .apply_kickoff_input(
                agent_identity,
                mob_dsl::MobMachineInput::KickoffClear {
                    member_id: agent_identity.to_string(),
                },
            )
            .await
        {
            Ok(true) => {
                self.roster.write().await.set_kickoff(agent_identity, None);
            }
            Ok(false) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %agent_identity,
                    "kickoff clear rejected by MobMachine; roster projection left unchanged"
                );
            }
            Err(error) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %agent_identity,
                    %error,
                    "kickoff clear failed"
                );
            }
        }
    }

    async fn fail_startup_to_stopped(&mut self, failure_label: &'static str) {
        if let Err(stop_error) = self.stop_all_autonomous_members().await {
            tracing::warn!(
                mob_id = %self.definition.id,
                error = %stop_error,
                "failed cleaning up autonomous host loops after startup error"
            );
        }
        if let Err(error) =
            self.apply_dsl_input(mob_dsl::MobMachineInput::Stop, "stop_after_startup_failure")
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                error = %error,
                failure_label,
                "authority rejected Stop after startup failure"
            );
        }
    }

    async fn apply_kickoff_input(
        &mut self,
        agent_identity: &MeerkatId,
        input: mob_dsl::MobMachineInput,
    ) -> Result<bool, MobError> {
        let transition = match mob_dsl::MobMachineMutator::apply(&mut self.dsl_authority, input) {
            Ok(transition) => transition,
            Err(_) => return Ok(false),
        };

        // Wave-c C-6p — harvest routed seam effects from this transition
        // into the pending-dispatch queue. Non-routed kickoff-specific
        // variants continue to be handled in the per-effect match below;
        // routed variants (`Request*`) are NOT silently dropped by the
        // prior `_ => {}` arm — they're lifted through
        // `lift_routed_effect` and await `flush_routed_effects`.
        self.queue_routed_effects_from(transition.effects());
        self.publish_machine_state_projection();

        for effect in transition.into_effects() {
            match effect {
                mob_dsl::MobMachineEffect::PersistKickoffUpdate {
                    member_id: _,
                    phase,
                } => {
                    let phase = Self::kickoff_phase_from_dsl(phase);
                    self.persist_kickoff_state(agent_identity, phase, None)
                        .await?;
                }
                mob_dsl::MobMachineEffect::PersistKickoffFailureUpdate {
                    member_id: _,
                    phase,
                    error,
                } => {
                    let phase = Self::kickoff_phase_from_dsl(phase);
                    self.persist_kickoff_state(agent_identity, phase, Some(error))
                        .await?;
                }
                mob_dsl::MobMachineEffect::EmitKickoffLifecycleNotice {
                    member_id: _,
                    intent,
                } => {
                    if let Some(notice_intent) = Self::kickoff_notice_intent(intent)
                        && let Err(error) = self
                            .notify_kickoff_event(agent_identity, notice_intent)
                            .await
                    {
                        tracing::warn!(
                            agent_identity = %agent_identity,
                            error = %error,
                            intent = %intent,
                            "failed to emit kickoff lifecycle notice"
                        );
                    }
                }
                // Routed seam effects (`Request*`) were harvested above
                // into `pending_routed_effects`; drain happens at the
                // next async boundary via `flush_routed_effects`.
                mob_dsl::MobMachineEffect::RequestRuntimeBinding { .. }
                | mob_dsl::MobMachineEffect::RequestRuntimeIngress { .. }
                | mob_dsl::MobMachineEffect::RequestPeerRuntimeIngress { .. }
                | mob_dsl::MobMachineEffect::RequestRuntimeRetire { .. }
                | mob_dsl::MobMachineEffect::RequestRuntimeDestroy { .. } => {}
                _ => {}
            }
        }

        Ok(true)
    }

    /// Count of in-flight flow runs tracked by the mob actor's shell,
    /// reported by test-only snapshots (`MobOrchestratorSnapshot`,
    /// `MobLifecycleSnapshot`). Wave-c WAR-1: this is shell-state
    /// introspection, not an authority read seam — `run_cancel_tokens`
    /// is the actor's own `BTreeMap<RunId, CancellationToken>` and
    /// carries no DSL semantics. Production callers on the
    /// `apply_in_phase` / `can_accept_in_phase` path (historical, see
    /// docs/architecture/machine-simplification-proposal.md) were
    /// removed earlier, leaving only the cfg-test snapshot sites; gate
    /// the method the same way its callers are gated so the
    /// `NoDeadAuthorityWiring` rule can drop the silencing allow
    /// without widening production reachability.
    #[cfg(test)]
    fn machine_active_run_count(&self) -> u32 {
        self.run_cancel_tokens.len() as u32
    }

    /// Project an observable orchestrator snapshot from DSL state, if this mob
    /// has an orchestrator. Returns `None` for plain mobs.
    #[cfg(test)]
    fn machine_orchestrator_snapshot(&self, phase: MobState) -> Option<MobOrchestratorSnapshot> {
        if !self.has_orchestrator {
            return None;
        }
        let coordinator_bound = self.dsl_authority.state().coordinator_bound;
        Some(MobOrchestratorSnapshot {
            phase,
            coordinator_bound,
            pending_spawn_count: self.dsl_authority.state().pending_spawn_count as u32,
            active_flow_count: self.machine_active_run_count(),
            // topology_revision and supervisor_active are shell diagnostics not
            // tracked by the DSL; project supervisor_active from coordinator_bound
            // to preserve existing test expectations.
            topology_revision: 0,
            supervisor_active: coordinator_bound,
        })
    }

    /// Guard that the mob is in one of the `allowed` phases.
    ///
    /// Used by command handlers that operate *within* the current state
    /// (retire, wire, external turn, etc.). The first allowed state is used
    /// as the `to` hint in the error.
    fn require_state(&self, allowed: &[MobState]) -> Result<(), MobError> {
        if allowed.contains(&self.state()) {
            Ok(())
        } else {
            Err(MobError::InvalidTransition {
                from: self.state(),
                to: allowed[0],
            })
        }
    }

    fn drain_completed_peer_delivery_tasks(&mut self) {
        while let Some(result) = self.peer_delivery_tasks.try_join_next() {
            match result {
                Ok(completion) => {
                    self.peer_delivery_inflight.remove(&completion.id);
                }
                Err(error) => {
                    tracing::debug!(error = %error, "peer delivery task failed");
                    if self.peer_delivery_tasks.is_empty() {
                        self.peer_delivery_inflight.clear();
                    }
                }
            }
        }
    }

    fn spawn_peer_message_delivery(
        &mut self,
        plan: PeerMessageDeliveryPlan,
        reply_tx: tokio::sync::oneshot::Sender<Result<meerkat_core::comms::SendReceipt, MobError>>,
    ) {
        let permit = match self.peer_delivery_permits.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let _ = reply_tx.send(Err(MobError::Internal(format!(
                    "peer message delivery lane saturated (limit={MAX_PENDING_PEER_DELIVERIES})"
                ))));
                return;
            }
        };
        let id = self.next_peer_delivery_ticket;
        self.next_peer_delivery_ticket = self.next_peer_delivery_ticket.wrapping_add(1);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.peer_delivery_inflight.insert(
            id,
            PeerDeliveryInflight {
                from: plan.from.clone(),
                to: plan.to.clone(),
                cancel_token: cancel_token.clone(),
            },
        );
        self.peer_delivery_tasks.spawn(async move {
            let _permit = permit;
            let result = tokio::select! {
                result = plan.sender_comms.send(plan.command) => result.map_err(MobError::from),
                () = cancel_token.cancelled() => Err(MobError::Internal(
                    "peer message delivery canceled because mob topology or lifecycle changed".to_string(),
                )),
            };
            let _ = reply_tx.send(result);
            PeerDeliveryCompletion { id }
        });
    }

    async fn cancel_pending_peer_deliveries(&mut self, reason: &'static str) {
        self.cancel_peer_deliveries_matching(reason, |_| true).await;
    }

    async fn cancel_peer_deliveries_for_member(
        &mut self,
        member: &MeerkatId,
        reason: &'static str,
    ) {
        self.cancel_peer_deliveries_matching(reason, |delivery| {
            &delivery.from == member || &delivery.to == member
        })
        .await;
    }

    async fn cancel_peer_deliveries_for_edge(
        &mut self,
        a: &MeerkatId,
        b: &MeerkatId,
        reason: &'static str,
    ) {
        self.cancel_peer_deliveries_matching(reason, |delivery| {
            (&delivery.from == a && &delivery.to == b) || (&delivery.from == b && &delivery.to == a)
        })
        .await;
    }

    async fn cancel_peer_deliveries_matching(
        &mut self,
        reason: &'static str,
        mut predicate: impl FnMut(&PeerDeliveryInflight) -> bool,
    ) {
        let ids = self
            .peer_delivery_inflight
            .iter()
            .filter_map(|(id, delivery)| predicate(delivery).then_some(*id))
            .collect::<BTreeSet<_>>();
        if ids.is_empty() {
            return;
        }
        tracing::debug!(
            mob_id = %self.definition.id,
            pending = ids.len(),
            reason,
            "canceling pending peer delivery tasks"
        );
        for id in &ids {
            if let Some(delivery) = self.peer_delivery_inflight.get(id) {
                delivery.cancel_token.cancel();
            }
        }
        while ids
            .iter()
            .any(|id| self.peer_delivery_inflight.contains_key(id))
        {
            match self.peer_delivery_tasks.join_next().await {
                Some(Ok(completion)) => {
                    self.peer_delivery_inflight.remove(&completion.id);
                }
                Some(Err(error)) => {
                    tracing::debug!(error = %error, "peer delivery task failed during cancellation");
                }
                None => break,
            }
        }
        for id in ids {
            self.peer_delivery_inflight.remove(&id);
        }
    }

    async fn notify_orchestrator_lifecycle(&mut self, message: String) {
        // Drain completed lifecycle tasks (non-blocking).
        while let Some(result) = self.lifecycle_tasks.try_join_next() {
            if let Err(error) = result {
                tracing::debug!(error = %error, "lifecycle notification task failed");
            }
        }

        let Some(orchestrator) = &self.definition.orchestrator else {
            return;
        };
        let Some(orchestrator_entry) = self
            .roster
            .read()
            .await
            .by_profile(&orchestrator.profile)
            .next()
            .cloned()
        else {
            return;
        };

        while self.lifecycle_tasks.len() >= MAX_LIFECYCLE_NOTIFICATION_TASKS {
            match self.lifecycle_tasks.join_next().await {
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    tracing::debug!(error = %error, "lifecycle notification task failed");
                }
                None => break,
            }
        }

        let provisioner = self.provisioner.clone();
        let runtime_mode = orchestrator_entry.runtime_mode;
        let agent_identity = orchestrator_entry.agent_identity;
        let bridge_session_id = self.machine_bridge_session_id_for_identity(&agent_identity);
        let Some(member_ref) = Self::project_member_ref_session_binding(
            &orchestrator_entry.member_ref,
            bridge_session_id,
        ) else {
            return;
        };
        self.lifecycle_tasks.spawn(async move {
            let result = match runtime_mode {
                crate::MobRuntimeMode::AutonomousHost => {
                    let Some(bridge_session_id) = member_ref.bridge_session_id() else {
                        return;
                    };
                    let Some(injector) = provisioner
                        .interaction_event_injector(bridge_session_id)
                        .await
                    else {
                        return;
                    };
                    injector
                        .inject(
                            message.into(),
                            meerkat_core::PlainEventSource::Rpc,
                            meerkat_core::types::HandlingMode::Queue,
                            None,
                        )
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "orchestrator lifecycle inject failed for '{agent_identity}': {error}"
                            ))
                        })
                }
                crate::MobRuntimeMode::TurnDriven => {
                    provisioner
                        .start_turn(
                            &member_ref,
                            meerkat_core::service::StartTurnRequest {
                                prompt: message.into(),
                                system_prompt: None,
                                event_tx: None,
                                runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
                            },
                        )
                        .await
                }
            };
            if let Err(error) = result {
                tracing::warn!(
                    orchestrator_member_ref = ?member_ref,
                    error = %error,
                    "failed to notify orchestrator lifecycle turn"
                );
            }
        });
    }

    fn retire_event_key(agent_identity: &MeerkatId, member_ref: &MemberRef) -> String {
        let member =
            serde_json::to_string(member_ref).unwrap_or_else(|_| format!("{member_ref:?}"));
        format!("{agent_identity}|{member}")
    }

    fn pending_spawn_maps_aligned(&self) -> bool {
        self.pending_spawn_alignment_violation().is_none()
    }

    fn pending_spawn_alignment_violation(&self) -> Option<String> {
        let expected = if self.has_orchestrator {
            Some(self.dsl_authority.state().pending_spawn_count as usize)
        } else {
            None
        };
        if let Some(message) = self.pending_spawns.alignment_violation(expected) {
            return Some(message);
        }
        if self.has_orchestrator {
            let dsl_pending = self
                .dsl_authority
                .state()
                .pending_spawn_sessions
                .iter()
                .map(|(identity, session_id)| (identity.0.clone(), session_id.0.clone()))
                .collect::<BTreeMap<_, _>>();
            let local_pending = self.pending_spawns.member_session_pairs();
            if dsl_pending != local_pending {
                return Some(format!(
                    "pending admission mismatch: dsl={dsl_pending:?}, local={local_pending:?}"
                ));
            }
        }
        None
    }

    fn ensure_pending_spawn_alignment(&self, context: &str) -> Result<(), MobError> {
        if let Some(message) = self.pending_spawn_alignment_violation() {
            return Err(MobError::Internal(format!(
                "{context}: pending spawn alignment violation: {message}"
            )));
        }
        Ok(())
    }

    fn debug_assert_pending_spawn_alignment(&self) {
        debug_assert!(
            self.pending_spawn_maps_aligned(),
            "pending spawn alignment must hold across pending maps and orchestrator count"
        );
    }

    fn insert_pending_spawn(
        &mut self,
        spawn_ticket: u64,
        pending: PendingSpawn,
        task: tokio::task::JoinHandle<()>,
    ) {
        let impact = self.pending_spawns.insert(spawn_ticket, pending, task);
        if let PendingSpawnInsertImpact::Collided { replaced_identity } = impact {
            // StageSpawn has already been accepted for the new slot in enqueue paths.
            // If we replaced a prior slot at the same ticket, close that prior
            // staged snapshot now so authority counters cannot drift silently.
            if let Some(replaced_identity) = replaced_identity.as_ref() {
                self.complete_orchestrator_spawn(
                    Some(spawn_ticket),
                    replaced_identity,
                    "pending spawn slot collision replaced existing entry",
                );
            }
            tracing::warn!(
                spawn_ticket,
                "pending spawn slot collision replaced existing entry"
            );
        }
        self.debug_assert_pending_spawn_alignment();
        if let Some(message) = self.pending_spawn_alignment_violation() {
            tracing::error!(
                spawn_ticket,
                message = %message,
                "pending spawn alignment violated after insert"
            );
        }
    }

    fn take_pending_spawn_slot(
        &mut self,
        spawn_ticket: u64,
    ) -> (Option<PendingSpawn>, Option<tokio::task::JoinHandle<()>>) {
        self.pending_spawns
            .take_slot(spawn_ticket)
            .map_or((None, None), |slot| (Some(slot.spawn), slot.task))
    }

    fn complete_pending_spawn_slot(
        &mut self,
        spawn_ticket: u64,
        context: &'static str,
    ) -> (Option<PendingSpawn>, Option<tokio::task::JoinHandle<()>>) {
        let (pending, task) = self.take_pending_spawn_slot(spawn_ticket);
        if pending.is_some() || task.is_some() {
            if let Some(pending) = pending.as_ref() {
                self.complete_orchestrator_spawn(
                    Some(spawn_ticket),
                    &pending.agent_identity,
                    context,
                );
            }
        }
        if let Some(message) = self.pending_spawn_alignment_violation() {
            tracing::error!(
                spawn_ticket,
                context,
                message = %message,
                "pending spawn alignment violated after completion"
            );
        }
        (pending, task)
    }

    fn stage_orchestrator_spawn(
        &mut self,
        agent_identity: &MeerkatId,
        session_id: &SessionId,
    ) -> Result<Option<SessionId>, MobError> {
        let dsl_agent_identity =
            mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(agent_identity.as_str()));
        let dsl_session_id = mob_dsl::SessionId::from_domain(session_id);
        let transition = self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::StageSpawn {
                agent_identity: dsl_agent_identity.clone(),
                session_id: dsl_session_id.clone(),
            },
            "stage_spawn",
        )?;
        let authorized = transition.effects().iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::PendingSpawnOperationOwnerAuthorized {
                    agent_identity: effect_identity,
                    session_id: effect_session_id,
                } if effect_identity == &dsl_agent_identity && effect_session_id == &dsl_session_id
            )
        });
        if !authorized {
            return Err(MobError::Internal(format!(
                "MobMachine StageSpawn did not authorize pending operation owner for '{agent_identity}'"
            )));
        }
        Ok(Some(session_id.clone()))
    }

    fn apply_generated_self_owned_operation_owner(
        provision_request: &mut ProvisionMemberRequest,
        generated_owner: Option<SessionId>,
    ) -> Result<(), MobError> {
        if !matches!(provision_request.binding, crate::RuntimeBinding::Session)
            || provision_request.owner_bridge_session_id.is_some()
            || provision_request.ops_registry.is_some()
        {
            return Ok(());
        }
        let Some(generated_owner) = generated_owner else {
            return Err(MobError::Internal(
                "session member operation requires generated pending spawn owner authority".into(),
            ));
        };
        provision_request.generated_self_owned_operation_owner = Some(generated_owner);
        Ok(())
    }

    fn preview_spawn_admission(
        &self,
        agent_identity: &MeerkatId,
        authorized_profile_material: &AuthorizedSpawnProfileMaterial,
        bridge_session_id: Option<&SessionId>,
    ) -> Result<(), MobError> {
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let replacing = self
            .dsl_authority
            .state()
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();

        self.preview_dsl_input(
            mob_dsl::MobMachineInput::Spawn {
                agent_identity: dsl_identity,
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                    &crate::ids::AgentRuntimeId::initial(domain_identity),
                ),
                fence_token: mob_dsl::FenceToken::from_domain(self.next_fence_token_preview()),
                generation: mob_dsl::Generation::from_domain(crate::ids::Generation::INITIAL),
                profile_material_digest: authorized_profile_material
                    .profile_material_digest
                    .clone(),
                external_addressable: authorized_profile_material.external_addressable,
                runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::AutonomousHost,
                bridge_session_id: bridge_session_id.map(mob_dsl::SessionId::from_domain),
                replacing,
            },
            "spawn_command_admission",
        )
        .map(|_| ())
        .map_err(|_| self.invalid_transition_to(MobState::Running))
    }

    fn preview_spawn_command_admission(&self, agent_identity: &MeerkatId) -> Result<(), MobError> {
        let _ = agent_identity;
        self.require_state(&[MobState::Running])
    }

    fn authorize_spawn_profile_material(
        &mut self,
        agent_identity: &MeerkatId,
        profile_name: &ProfileName,
        profile: &crate::profile::Profile,
        context: &str,
    ) -> Result<AuthorizedSpawnProfileMaterial, MobError> {
        let (input, expected) =
            authorize_spawn_profile_input(agent_identity, profile_name, profile)?;
        let transition = self.apply_dsl_input_collect_transition(input, context)?;
        require_authorized_effect(&transition, &expected, context)?;
        Ok(expected)
    }

    fn preview_run_flow_command_admission(&self, run_id: &RunId) -> Result<(), MobError> {
        self.prepare_command_admission(
            mob_dsl::MobMachineInput::RunFlow {
                run_id: mob_dsl::RunId::from(run_id.to_string()),
                step_ids: Default::default(),
                ordered_steps: Vec::new(),
                step_status: Default::default(),
                output_recorded: Default::default(),
                step_condition_results: Default::default(),
                step_has_conditions: Default::default(),
                step_dependencies: Default::default(),
                step_dependency_modes: Default::default(),
                step_branches: Default::default(),
                step_collection_policies: Default::default(),
                step_quorum_thresholds: Default::default(),
                step_target_counts: Default::default(),
                step_target_success_counts: Default::default(),
                step_target_terminal_failure_counts: Default::default(),
                escalation_threshold: 0,
                max_step_retries: 0,
                max_active_nodes: 0,
                max_active_frames: 0,
                max_frame_depth: 0,
            },
            MobState::Running,
            "run_flow_command_admission",
        )
        .map(|_| ())
    }

    fn resolve_respawn_topology_restore_result(
        &mut self,
        agent_identity: &MeerkatId,
        failed_restore_peer_ids: Vec<RespawnTopologyPeerId>,
    ) -> Result<RespawnTopologyRestoreResolution, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity::from(agent_identity.as_str());
        let dsl_failed_peer_ids = failed_restore_peer_ids
            .iter()
            .map(|peer_id| mob_dsl::RespawnTopologyPeerId::from(peer_id.as_str()))
            .collect::<Vec<_>>();
        let transition = self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::ResolveRespawnTopologyRestore {
                agent_identity: dsl_identity.clone(),
                failed_peer_ids: dsl_failed_peer_ids.clone(),
            },
            "respawn_topology_restore_result",
        )?;
        let resolution = transition
            .into_effects()
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::RespawnTopologyRestoreResolved {
                    agent_identity: effect_identity,
                    result,
                    failed_peer_ids,
                } if effect_identity == dsl_identity => Some((result, failed_peer_ids)),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted respawn topology feedback but emitted no typed result"
                        .into(),
                )
            })?;
        let (result, effect_failed_peer_ids) = resolution;
        if effect_failed_peer_ids != dsl_failed_peer_ids {
            return Err(MobError::Internal(
                "MobMachine respawn topology feedback echoed inconsistent failed peer ids".into(),
            ));
        }
        let failed_peer_ids = effect_failed_peer_ids
            .into_iter()
            .map(|peer_id| RespawnTopologyPeerId::from(peer_id.0.as_str()))
            .collect::<Vec<_>>();
        match result {
            mob_dsl::RespawnTopologyRestoreResultKind::Completed if failed_peer_ids.is_empty() => {
                Ok(RespawnTopologyRestoreResolution {
                    result,
                    failed_peer_ids,
                })
            }
            mob_dsl::RespawnTopologyRestoreResultKind::Completed => Err(MobError::Internal(
                "MobMachine classified respawn topology restore as completed with failed peers"
                    .into(),
            )),
            mob_dsl::RespawnTopologyRestoreResultKind::TopologyRestoreFailed
                if !failed_peer_ids.is_empty() =>
            {
                Ok(RespawnTopologyRestoreResolution {
                    result,
                    failed_peer_ids,
                })
            }
            mob_dsl::RespawnTopologyRestoreResultKind::TopologyRestoreFailed => {
                Err(MobError::Internal(
                    "MobMachine classified respawn topology restore as failed without failed peers"
                        .into(),
                ))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_submit_work_rejection_in_authority(
        authority: &mut mob_dsl::MobMachineAuthority,
        dsl_identity: &mob_dsl::AgentIdentity,
        dsl_runtime_id: &mob_dsl::AgentRuntimeId,
        dsl_fence_token: mob_dsl::FenceToken,
        runtime_id: &AgentRuntimeId,
        origin: WorkOrigin,
        agent_identity: &MeerkatId,
        current_state: MobState,
    ) -> MobError {
        let dsl_origin = mob_dsl::WorkOrigin::from(origin);
        let transition = match mob_dsl::MobMachineMutator::apply(
            authority,
            mob_dsl::MobMachineInput::ResolveSubmitWorkRejection {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: dsl_runtime_id.clone(),
                fence_token: dsl_fence_token,
                origin: dsl_origin,
            },
        ) {
            Ok(transition) => transition,
            Err(err) => {
                return MobError::Internal(format!(
                    "MobMachine rejected SubmitWork and failed to resolve typed rejection: {err}"
                ));
            }
        };
        let reason = transition
            .into_effects()
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::SubmitWorkRejected {
                    agent_runtime_id,
                    reason,
                    expected_fence_token,
                    actual_fence_token,
                    ..
                } if agent_runtime_id == *dsl_runtime_id => {
                    Some((reason, expected_fence_token, actual_fence_token))
                }
                _ => None,
            });
        match reason {
            Some((mob_dsl::SubmitWorkRejectReasonKind::MobNotRunning, _, _)) => {
                MobError::InvalidTransition {
                    from: current_state,
                    to: MobState::Running,
                }
            }
            Some((mob_dsl::SubmitWorkRejectReasonKind::MemberNotFound, _, _)) => {
                MobError::MemberNotFound(agent_identity.clone())
            }
            Some((
                mob_dsl::SubmitWorkRejectReasonKind::StaleFenceToken,
                Some(expected),
                Some(actual),
            )) => MobError::StaleFenceToken {
                runtime_id: runtime_id.clone(),
                expected: FenceToken::new(expected.0),
                actual: FenceToken::new(actual.0),
            },
            Some((mob_dsl::SubmitWorkRejectReasonKind::StaleFenceToken, _, _)) => {
                MobError::Internal(
                    "MobMachine rejected SubmitWork as stale without fence-token feedback".into(),
                )
            }
            Some((mob_dsl::SubmitWorkRejectReasonKind::NotExternallyAddressable, _, _)) => {
                MobError::NotExternallyAddressable(agent_identity.clone())
            }
            None => MobError::Internal(
                "MobMachine rejected SubmitWork without typed rejection feedback".into(),
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_submit_work_projection_missing_or_rejection(
        authority: &mut mob_dsl::MobMachineAuthority,
        declared_submit_work_admitted: bool,
        dsl_identity: &mob_dsl::AgentIdentity,
        dsl_runtime_id: &mob_dsl::AgentRuntimeId,
        dsl_fence_token: mob_dsl::FenceToken,
        runtime_id: &AgentRuntimeId,
        origin: WorkOrigin,
        agent_identity: &MeerkatId,
        current_state: MobState,
    ) -> MobError {
        if declared_submit_work_admitted {
            return MobError::Internal(format!(
                "MobMachine admitted SubmitWork for '{agent_identity}' but the roster projection has no member entry"
            ));
        }
        Self::resolve_submit_work_rejection_in_authority(
            authority,
            dsl_identity,
            dsl_runtime_id,
            dsl_fence_token,
            runtime_id,
            origin,
            agent_identity,
            current_state,
        )
    }

    async fn resolve_spawn_policy_via_machine(
        &mut self,
        identity: &AgentIdentity,
    ) -> Result<Option<super::spawn_policy::SpawnSpec>, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        let (policy_enabled, revision) = {
            let state = self.dsl_authority.state();
            (state.spawn_policy_enabled, state.spawn_policy_revision)
        };
        if !policy_enabled {
            return Ok(None);
        }

        let observed = self.spawn_policy.observe_resolution(identity).await;
        let profile_name = observed
            .as_ref()
            .map(|spec| spec.profile.as_str().to_owned());
        let runtime_mode = observed
            .as_ref()
            .and_then(|spec| spec.runtime_mode.map(mob_dsl::SpawnPolicyRuntimeMode::from));

        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::ResolveSpawnPolicy {
                agent_identity: dsl_identity.clone(),
                revision,
                profile_name,
                runtime_mode,
            },
            "resolve_spawn_policy",
        )?;

        let recorded = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::SpawnPolicyResolutionRecorded {
                agent_identity,
                revision: effect_revision,
                profile_name,
                runtime_mode,
            } if *agent_identity == dsl_identity && *effect_revision == revision => {
                Some((profile_name.clone(), *runtime_mode))
            }
            _ => None,
        });

        match recorded {
            Some((Some(profile), runtime_mode)) => Ok(Some(super::spawn_policy::SpawnSpec {
                profile: crate::ids::ProfileName::from(profile),
                runtime_mode: runtime_mode.map(crate::MobRuntimeMode::from),
            })),
            Some((None, None)) => Ok(None),
            Some((None, Some(_))) => Err(MobError::Internal(
                "MobMachine recorded spawn-policy runtime mode without profile".into(),
            )),
            None => Err(MobError::Internal(
                "MobMachine accepted spawn-policy resolution but emitted no typed feedback".into(),
            )),
        }
    }

    fn resolve_cancel_all_work_rejection_in_authority(
        authority: &mut mob_dsl::MobMachineAuthority,
        dsl_identity: &mob_dsl::AgentIdentity,
        dsl_runtime_id: &mob_dsl::AgentRuntimeId,
        dsl_fence_token: mob_dsl::FenceToken,
        runtime_id: &AgentRuntimeId,
        agent_identity: &MeerkatId,
        current_state: MobState,
    ) -> MobError {
        let transition = match mob_dsl::MobMachineMutator::apply(
            authority,
            mob_dsl::MobMachineInput::ResolveCancelAllWorkRejection {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: dsl_runtime_id.clone(),
                fence_token: dsl_fence_token,
            },
        ) {
            Ok(transition) => transition,
            Err(err) => {
                return MobError::Internal(format!(
                    "MobMachine rejected CancelAllWork and failed to resolve typed rejection: {err}"
                ));
            }
        };
        let reason = transition
            .into_effects()
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::CancelAllWorkRejected {
                    agent_runtime_id,
                    reason,
                    expected_fence_token,
                    actual_fence_token,
                } if agent_runtime_id == *dsl_runtime_id => {
                    Some((reason, expected_fence_token, actual_fence_token))
                }
                _ => None,
            });
        match reason {
            Some((mob_dsl::CancelAllWorkRejectReasonKind::MobNotRunning, _, _)) => {
                MobError::InvalidTransition {
                    from: current_state,
                    to: MobState::Running,
                }
            }
            Some((mob_dsl::CancelAllWorkRejectReasonKind::MemberNotFound, _, _)) => {
                MobError::MemberNotFound(agent_identity.clone())
            }
            Some((
                mob_dsl::CancelAllWorkRejectReasonKind::StaleFenceToken,
                Some(expected),
                Some(actual),
            )) => MobError::StaleFenceToken {
                runtime_id: runtime_id.clone(),
                expected: FenceToken::new(expected.0),
                actual: FenceToken::new(actual.0),
            },
            Some((mob_dsl::CancelAllWorkRejectReasonKind::StaleFenceToken, _, _)) => {
                MobError::Internal(
                    "MobMachine rejected CancelAllWork as stale without fence-token feedback"
                        .into(),
                )
            }
            None => MobError::Internal(
                "MobMachine rejected CancelAllWork without typed rejection feedback".into(),
            ),
        }
    }

    fn preview_policy_spawn_submit_work_admission(
        &self,
        agent_identity: &MeerkatId,
        authorized_profile_material: &AuthorizedSpawnProfileMaterial,
        work_ref: &WorkRef,
        origin: WorkOrigin,
    ) -> Result<(), MobError> {
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let domain_runtime_id =
            crate::ids::AgentRuntimeId::new(domain_identity, crate::ids::Generation::INITIAL);
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&domain_runtime_id);
        let dsl_fence_token = mob_dsl::FenceToken::from_domain(self.next_fence_token_preview());
        let replacing = self
            .dsl_authority
            .state()
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();
        let mut authority =
            mob_dsl::MobMachineAuthority::recover_from_state(self.dsl_authority.state().clone())
                .map_err(|error| {
                    MobError::Internal(format!(
                        "spawn preview could not recover DSL authority state: {error}"
                    ))
                })?;
        let spawn = mob_dsl::MobMachineInput::Spawn {
            agent_identity: dsl_identity.clone(),
            agent_runtime_id: dsl_runtime_id.clone(),
            fence_token: dsl_fence_token,
            generation: mob_dsl::Generation::from_domain(crate::ids::Generation::INITIAL),
            profile_material_digest: authorized_profile_material.profile_material_digest.clone(),
            external_addressable: authorized_profile_material.external_addressable,
            runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::AutonomousHost,
            bridge_session_id: Some(mob_dsl::SessionId::from_domain(&SessionId::new())),
            replacing,
        };
        let transition = mob_dsl::MobMachineMutator::apply(&mut authority, spawn)
            .map_err(|_| self.invalid_transition_to(MobState::Running))?;
        let _ = transition;

        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::SubmitWork {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: dsl_runtime_id.clone(),
                fence_token: dsl_fence_token,
                work_id: mob_dsl::WorkId::from_work_ref(work_ref),
                origin: mob_dsl::WorkOrigin::from(origin),
            },
        )
        .map(|_| ())
        .map_err(|_| {
            Self::resolve_submit_work_rejection_in_authority(
                &mut authority,
                &dsl_identity,
                &dsl_runtime_id,
                dsl_fence_token,
                &domain_runtime_id,
                origin,
                agent_identity,
                self.state(),
            )
        })
    }

    fn complete_orchestrator_spawn(
        &mut self,
        spawn_ticket: Option<u64>,
        agent_identity: &MeerkatId,
        context: &'static str,
    ) {
        if let Err(error) = self.apply_dsl_signal(
            mob_dsl::MobMachineSignal::CompleteSpawn {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(
                    agent_identity.as_str(),
                )),
            },
            "complete_spawn",
        ) {
            if let Some(spawn_ticket) = spawn_ticket {
                tracing::warn!(
                    spawn_ticket,
                    agent_identity = %agent_identity,
                    error = %error,
                    context,
                    "failed to reconcile generated pending-spawn snapshot"
                );
            } else {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    error = %error,
                    context,
                    "failed to reconcile generated pending-spawn snapshot"
                );
            }
        }
    }

    async fn flow_tracker_alignment_violation(&self) -> Option<String> {
        let snapshot = self.flow_tracker_snapshot().await;
        let run_task_ids = snapshot.run_task_ids;
        let run_token_ids = snapshot.cancel_token_ids;
        if run_task_ids != run_token_ids {
            return Some(format!(
                "run task/token tracker mismatch: tasks={run_task_ids:?}, tokens={run_token_ids:?}"
            ));
        }

        let stream_ids = snapshot.stream_ids;
        let unknown_streams = stream_ids
            .iter()
            .filter(|run_id| !run_task_ids.contains(*run_id))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown_streams.is_empty() {
            return Some(format!(
                "flow stream tracker contains unknown runs: {unknown_streams:?}"
            ));
        }

        None
    }

    async fn flow_tracker_snapshot(&self) -> super::MobFlowTrackerSnapshot {
        super::MobFlowTrackerSnapshot {
            run_task_ids: self
                .run_tasks
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            cancel_token_ids: self
                .run_cancel_tokens
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            stream_ids: self
                .flow_streams
                .lock()
                .await
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            tracked_flows: self
                .run_cancel_tokens
                .iter()
                .map(|(run_id, (_, flow_id))| (run_id.clone(), flow_id.clone()))
                .collect(),
        }
    }

    async fn ensure_flow_tracker_alignment(&self, context: &str) -> Result<(), MobError> {
        if let Some(message) = self.flow_tracker_alignment_violation().await {
            return Err(MobError::Internal(format!(
                "{context}: flow tracker alignment violation: {message}"
            )));
        }
        Ok(())
    }

    async fn cleanup_namespace(&self) -> Result<(), MobError> {
        Ok(())
    }

    fn fallback_spawn_prompt(
        &self,
        profile_name: &ProfileName,
        agent_identity: &MeerkatId,
    ) -> String {
        format!(
            "You have been spawned as '{}' (role: {}) in mob '{}'.",
            agent_identity, profile_name, self.definition.id
        )
    }

    /// Start the autonomous runtime for a member and deliver its initial prompt.
    ///
    /// Sets up the keep-alive infrastructure (comms drain, dispatch capability)
    /// then delivers the prompt as a normal turn. The session was created with
    /// `InitialTurnPolicy::Defer` so the prompt hasn't been executed yet.
    ///
    /// Two paths:
    /// - **Runtime-backed (adapter present):** Builds `Input::Prompt` and calls
    ///   `accept_input_with_completion` for a true admission ack. Spawns a
    ///   background task for completion wait + barrier signal.
    /// - **No adapter (test/ephemeral):** Falls back to `provisioner.start_turn()`
    ///   in a spawned task with yield-check for immediate failure detection.
    #[cfg(feature = "runtime-adapter")]
    async fn start_autonomous_member(
        &mut self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
        prompt: meerkat_core::types::ContentInput,
    ) -> Result<(), MobError> {
        self.ensure_autonomous_runtime_ready(agent_identity, member_ref)
            .await?;

        let startup_marker = {
            let roster = self.roster.read().await;
            roster
                .get_by_identity(&AgentIdentity::from(agent_identity.as_str()))
                .map(|entry| {
                    (
                        mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
                        mob_dsl::FenceToken::from_domain(entry.fence_token),
                    )
                })
        }
        .ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{agent_identity}' missing roster entry for startup readiness"
            ))
        })?;

        if !self
            .dsl_authority
            .state()
            .member_startup_ready
            .contains(&startup_marker.0)
        {
            self.apply_dsl_input(
                mob_dsl::MobMachineInput::StartupMarkReady {
                    agent_runtime_id: startup_marker.0,
                    fence_token: startup_marker.1,
                },
                "start_autonomous_member/startup_mark_ready",
            )?;
        }

        let bridge_session_id = member_ref.bridge_session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{agent_identity}' must be session-backed"
            ))
        })?;

        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{agent_identity}' requires admission-capable substrate (runtime adapter)"
            ))
        })?;

        {
            // Runtime-backed path: true admission ack via accept_input_with_completion.
            use meerkat_runtime::{Input, InputHeader, PromptInput};

            let input = Input::Prompt(PromptInput {
                header: InputHeader {
                    id: meerkat_core::lifecycle::InputId::new(),
                    timestamp: chrono::Utc::now(),
                    source: meerkat_runtime::InputOrigin::Operator,
                    durability: meerkat_runtime::InputDurability::Durable,
                    visibility: meerkat_runtime::InputVisibility::default(),
                    idempotency_key: None,
                    supersession_key: None,
                    correlation_id: None,
                },
                text: prompt.text_content(),
                blocks: if prompt.has_images() {
                    Some(prompt.into_blocks())
                } else {
                    None
                },
                typed_turn_appends: Vec::new(),
                turn_metadata: None,
            });

            let (_outcome, completion_handle) = adapter
                .accept_input_with_completion(bridge_session_id, input)
                .await
                .map_err(|e| {
                    MobError::Internal(format!(
                        "autonomous prompt admission failed for '{agent_identity}': {e}"
                    ))
                })?;

            // Spawn background task for completion wait.
            let log_id = agent_identity.clone();
            let completion_command_tx = self.command_tx.clone();
            let handle = tokio::spawn(async move {
                if let Some(h) = completion_handle {
                    let outcome = h.wait().await;
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if completion_command_tx
                        .send(MobCommand::KickoffOutcomeResolved {
                            agent_identity: log_id.clone(),
                            outcome,
                            ack_tx,
                        })
                        .await
                        .is_err()
                    {
                        tracing::warn!(
                            agent_identity = %log_id,
                            "mob actor dropped before kickoff outcome could be recorded"
                        );
                    } else {
                        let _ = ack_rx.await;
                    }
                }
            });

            self.autonomous_initial_turns
                .lock()
                .await
                .insert(agent_identity.clone(), InitialTurnHandle { handle });
        }

        tracing::debug!(agent_identity = %agent_identity, "autonomous member started");
        Ok(())
    }

    async fn ensure_autonomous_runtime_ready(
        &self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        // Session registration + RuntimeLoop attachment is owned by the
        // provisioner's lazy `runtime_session_state()` init (called during
        // provision_member). stop_autonomous_member preserves registration
        // (only aborts the drain), so resume just needs to re-spawn the drain.
        self.ensure_mob_comms_drain(agent_identity, member_ref)
            .await?;

        self.ensure_autonomous_dispatch_capability(agent_identity, member_ref)
            .await
    }

    async fn ensure_mob_comms_drain(
        &self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
        {
            let Some(bridge_session_id) = member_ref.bridge_session_id() else {
                return Ok(());
            };

            if let Some(adapter) = self.runtime_adapter.clone()
                && let Some(comms_runtime) = self.provisioner.comms_runtime(member_ref).await
            {
                let mob_id =
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(self.definition.id.as_ref());
                let spawned = adapter
                    .maybe_spawn_mob_comms_drain(bridge_session_id, comms_runtime, mob_id)
                    .await;
                if spawned {
                    tracing::debug!(
                        agent_identity = %agent_identity,
                        session_id = %bridge_session_id,
                        "updated peer ingress for mob member"
                    );
                }
            }
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "runtime-adapter")))]
        {
            let _ = (agent_identity, member_ref);
        }

        Ok(())
    }

    async fn teardown_session_runtime_bindings_from_machine(&self) {
        #[cfg(feature = "runtime-adapter")]
        if let Some(adapter) = &self.runtime_adapter {
            let session_ids = self
                .dsl_authority
                .state()
                .member_session_bindings
                .values()
                .filter_map(|session_id| SessionId::parse(&session_id.0).ok())
                .collect::<Vec<_>>();
            for session_id in session_ids {
                adapter.abort_comms_drain(&session_id).await;
                adapter.unregister_session(&session_id).await;
            }
        }
    }

    async fn ensure_autonomous_dispatch_capability_for_provisioner(
        provisioner: &Arc<dyn MobProvisioner>,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        let bridge_session_id = member_ref.bridge_session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{agent_identity}' must be session-backed for injector dispatch"
            ))
        })?;
        if provisioner
            .interaction_event_injector(bridge_session_id)
            .await
            .is_none()
        {
            return Err(MobError::MissingMemberCapability {
                member_id: agent_identity.clone(),
                capability: crate::error::MobMemberCapability::InteractionEventInjector,
                context: "autonomous member dispatch",
            });
        }
        Ok(())
    }

    #[cfg(feature = "runtime-adapter")]
    async fn resolve_kickoff_outcome(
        &mut self,
        agent_identity: &MeerkatId,
        outcome: Result<
            meerkat_runtime::completion::CompletionOutcome,
            meerkat_runtime::completion::CompletionWaitError,
        >,
    ) -> Result<(), MobError> {
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                return self
                    .apply_kickoff_input(
                        agent_identity,
                        mob_dsl::MobMachineInput::KickoffResolveFailed {
                            member_id: agent_identity.to_string(),
                            error: format!("runtime completion waiter failed: {error}"),
                        },
                    )
                    .await
                    .map(|_| ());
            }
        };

        if let meerkat_runtime::completion::CompletionOutcome::CallbackPending { tool_name, args } =
            &outcome
        {
            tracing::debug!(
                agent_identity = %agent_identity,
                tool_name = %tool_name,
                args = ?args,
                "autonomous kickoff reached callback-pending boundary"
            );
        }

        let _ = self
            .apply_kickoff_input(
                agent_identity,
                match outcome {
                    meerkat_runtime::completion::CompletionOutcome::Completed(_)
                    | meerkat_runtime::completion::CompletionOutcome::CompletedWithFinalizationFailure {
                        ..
                    }
                    | meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => {
                        mob_dsl::MobMachineInput::KickoffResolveStarted {
                            member_id: agent_identity.to_string(),
                        }
                    }
                    meerkat_runtime::completion::CompletionOutcome::CallbackPending { .. } => {
                        mob_dsl::MobMachineInput::KickoffResolveCallbackPending {
                            member_id: agent_identity.to_string(),
                        }
                    }
                    meerkat_runtime::completion::CompletionOutcome::Cancelled => {
                        mob_dsl::MobMachineInput::KickoffCancelRequested {
                            member_id: agent_identity.to_string(),
                        }
                    }
                    meerkat_runtime::completion::CompletionOutcome::Abandoned(error)
                    | meerkat_runtime::completion::CompletionOutcome::AbandonedWithError {
                        reason: error,
                        ..
                    }
                    | meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(error) => {
                        mob_dsl::MobMachineInput::KickoffResolveFailed {
                            member_id: agent_identity.to_string(),
                            error,
                        }
                    }
                },
            )
            .await?;
        Ok(())
    }

    async fn maybe_mark_kickoff_cancelled(&mut self, agent_identity: &MeerkatId) {
        if let Err(error) = self
            .apply_kickoff_input(
                agent_identity,
                mob_dsl::MobMachineInput::KickoffCancelRequested {
                    member_id: agent_identity.to_string(),
                },
            )
            .await
        {
            tracing::warn!(
                agent_identity = %agent_identity,
                error = %error,
                "failed to apply kickoff cancellation transition"
            );
        }
    }

    async fn ensure_autonomous_dispatch_capability(
        &self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        Self::ensure_autonomous_dispatch_capability_for_provisioner(
            &self.provisioner,
            agent_identity,
            member_ref,
        )
        .await
    }

    /// Stop an autonomous member: abort initial turn, interrupt, abort comms drain.
    ///
    /// Does NOT unregister the session — that happens only on dispose (retire/destroy).
    /// This allows resume to re-spawn the comms drain without re-registering.
    async fn stop_autonomous_member(
        &mut self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        // Abort any in-flight initial turn.
        let had_kickoff_handle = self
            .autonomous_initial_turns
            .lock()
            .await
            .contains_key(agent_identity);
        if had_kickoff_handle {
            self.maybe_mark_kickoff_cancelled(agent_identity).await;
        }
        if let Some(handle) = self
            .autonomous_initial_turns
            .lock()
            .await
            .remove(agent_identity)
        {
            handle.abort();
        }
        if let Err(error) = self.provisioner.interrupt_member(member_ref).await
            && !matches!(
                error,
                MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
            )
        {
            return Err(error);
        }
        // Abort the comms drain but keep the session registered.
        #[cfg(feature = "runtime-adapter")]
        if let (Some(adapter), Some(session_id)) =
            (&self.runtime_adapter, member_ref.bridge_session_id())
        {
            adapter.abort_comms_drain(session_id).await;
        }
        // Ensure stop semantics are strong: do not report completion while the
        // session still appears active, otherwise immediate resume can race into
        // SessionError::Busy.
        let mut still_active = false;
        for _ in 0..40 {
            match self.provisioner.is_member_active(member_ref).await? {
                Some(true) => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
                _ => {
                    still_active = false;
                    break;
                }
            }
            still_active = true;
        }
        if still_active {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                "autonomous member stop polling exhausted before member became idle"
            );
        }
        Ok(())
    }

    async fn stop_all_autonomous_members(&mut self) -> Result<(), MobError> {
        let entries = {
            let roster = self.roster.read().await;
            roster
                .list()
                .filter(|entry| entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost)
                .cloned()
                .collect::<Vec<_>>()
        };
        if entries.is_empty() {
            return Ok(());
        }
        let mut first_error: Option<MobError> = None;
        for entry in entries {
            let result = self.stop_autonomous_member_entry(entry).await;
            if let Err((agent_identity, error)) = result {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    error = %error,
                    "failed stopping autonomous member"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn stop_autonomous_member_entry(
        &mut self,
        entry: RosterEntry,
    ) -> Result<(), (MeerkatId, MobError)> {
        self.stop_autonomous_member(&entry.agent_identity, &entry.member_ref)
            .await
            .map_err(|error| (entry.agent_identity, error))
    }

    /// Ensure all autonomous roster members have their runtime ready.
    ///
    /// Called on mob startup and resume. Does NOT fire synthetic kickoff turns —
    /// the keep-alive runtime infrastructure is sufficient. On resume, the
    /// member's session already has its conversation history.
    async fn ensure_autonomous_runtimes_from_roster(&self) -> Result<(), MobError> {
        let broken_members = self
            .dsl_authority
            .state()
            .member_restore_failures
            .keys()
            .map(|identity| MeerkatId::from(identity.0.as_str()))
            .collect::<HashSet<_>>();
        let entries = {
            let roster = self.roster.read().await;
            roster
                .list()
                .filter(|entry| {
                    entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost
                        && !broken_members.contains(&entry.agent_identity)
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        if entries.is_empty() {
            // Turn-driven resumed members still need their mob-owned comms
            // drain rebound even though they do not need autonomous dispatch.
        }

        let mut first_error: Option<MobError> = None;
        let all_entries = {
            let roster = self.roster.read().await;
            roster
                .list()
                .filter(|entry| {
                    entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost
                        && !broken_members.contains(&entry.agent_identity)
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        for entry in &all_entries {
            let ensure_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                self.provisioner
                    .ensure_runtime_session_state(&entry.member_ref)
                    .await?;
                self.ensure_mob_comms_drain(&entry.agent_identity, &entry.member_ref)
                    .await
            })
            .await;
            let result = match ensure_result {
                Ok(result) => result,
                Err(_elapsed) => {
                    tracing::warn!(
                        agent_identity = %entry.agent_identity,
                        "timed out ensuring mob comms drain ready"
                    );
                    continue;
                }
            };
            if let Err(error) = result {
                tracing::warn!(
                    agent_identity = %entry.agent_identity,
                    error = %error,
                    "failed ensuring mob comms drain ready"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        for entry in &entries {
            let ensure_result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.ensure_autonomous_runtime_ready(&entry.agent_identity, &entry.member_ref),
            )
            .await;
            let result = match ensure_result {
                Ok(result) => result,
                Err(_elapsed) => {
                    tracing::warn!(
                        agent_identity = %entry.agent_identity,
                        "timed out ensuring autonomous runtime ready"
                    );
                    continue;
                }
            };
            if let Err(error) = result {
                tracing::warn!(
                    agent_identity = %entry.agent_identity,
                    error = %error,
                    "failed ensuring autonomous runtime ready"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    /// Main actor loop: process commands sequentially until Shutdown.
    pub(super) async fn run(mut self, mut command_rx: mpsc::Receiver<MobCommand>) {
        if matches!(self.dsl_state(), MobState::Running) {
            if let Err(error) = self.restore_generated_member_operation_bindings().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    "failed to restore generated mob member operation bindings during actor startup; entering Stopped"
                );
                self.fail_startup_to_stopped("mob member operation binding restore failure")
                    .await;
            }
        }
        if matches!(self.state(), MobState::Running) {
            if let Err(error) = self.ensure_autonomous_runtimes_from_roster().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    "failed to start autonomous host loops during actor startup; entering Stopped"
                );
                self.fail_startup_to_stopped("autonomous runtime startup failure")
                    .await;
            }
        }
        let mut deferred_commands = VecDeque::new();
        loop {
            self.drain_completed_peer_delivery_tasks();
            let cmd = if let Some(cmd) = deferred_commands.pop_front() {
                cmd
            } else if let Some(cmd) = command_rx.recv().await {
                cmd
            } else {
                break;
            };
            tracing::debug!(command_kind = cmd.kind(), "MobActor received command");
            match cmd {
                MobCommand::Spawn {
                    spec,
                    spawn_source,
                    owner_bridge_session_id,
                    ops_registry,
                    reply_tx,
                } => {
                    tracing::debug!(
                        member_id = %spec.identity,
                        profile = %spec.role_name,
                        owner_bound = owner_bridge_session_id.is_some(),
                        "MobActor received Spawn command"
                    );
                    Box::pin(self.enqueue_spawn(
                        *spec,
                        spawn_source,
                        owner_bridge_session_id,
                        ops_registry,
                        reply_tx,
                    ))
                    .await;
                }
                MobCommand::SpawnProvisioned {
                    spawn_ticket,
                    result,
                } => {
                    let mut completions = vec![(spawn_ticket, result)];
                    loop {
                        match command_rx.try_recv() {
                            Ok(MobCommand::SpawnProvisioned {
                                spawn_ticket,
                                result,
                            }) => completions.push((spawn_ticket, result)),
                            Ok(other) => {
                                deferred_commands.push_back(other);
                                break;
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                        }
                    }
                    // Box::pin: the `handle_spawn_provisioned_batch`
                    // future tips past clippy's `large_futures` 16 KiB
                    // threshold after Track-B R5 added
                    // `peer_projection_*` state to `MeerkatMachine` —
                    // the enum size cascades through the actor's
                    // transitive borrows. Heap-allocating the future
                    // keeps the polling stack frame small without
                    // reshaping the handler.
                    Box::pin(self.handle_spawn_provisioned_batch(completions)).await;
                }
                MobCommand::Retire {
                    agent_identity,
                    reply_tx,
                } => {
                    let result = match self
                        .cancel_pending_spawns_for_member(
                            &agent_identity,
                            "retire command received",
                        )
                        .await
                    {
                        Ok(canceled) => {
                            if canceled > 0 {
                                tracing::info!(
                                    agent_identity = %agent_identity,
                                    canceled,
                                    "retire canceled pending spawn lineage before roster retirement"
                                );
                            }
                            self.handle_retire(agent_identity).await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Respawn {
                    agent_identity,
                    initial_message,
                    reply_tx,
                } => {
                    let result =
                        Box::pin(self.handle_respawn(agent_identity, initial_message)).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::RetireAll { reply_tx } => {
                    let result = self.retire_all_members("retire_all").await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::SubmitWork { payload, reply_tx } => {
                    tracing::debug!(
                        agent_identity = %payload.runtime_id.identity,
                        runtime_id = %payload.runtime_id,
                        work_ref = %payload.work_ref,
                        origin = ?payload.origin,
                        handling_mode = ?payload.handling_mode,
                        ack_mode = ?payload.ack_mode,
                        "MobActor handling SubmitWork command"
                    );
                    match Box::pin(self.handle_submit_work(payload)).await {
                        Ok(SubmitWorkDispatchCompletion::Completed) => {
                            let _ = reply_tx.send(Ok(()));
                        }
                        Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                            operation_id: _,
                            member_ref,
                            req,
                        }) => {
                            Self::spawn_turn_admission_reply(
                                self.provisioner.clone(),
                                member_ref,
                                req,
                                reply_tx,
                            );
                        }
                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            member_ref,
                            req,
                        }) => {
                            Self::spawn_turn_completed_reply(
                                self.provisioner.clone(),
                                member_ref,
                                req,
                                reply_tx,
                            );
                        }
                        Err(error) => {
                            let _ = reply_tx.send(Err(error));
                        }
                    }
                }
                MobCommand::SendPeerMessage {
                    from,
                    to,
                    content,
                    handling_mode,
                    reply_tx,
                } => {
                    match Box::pin(self.prepare_send_peer_message(from, to, content, handling_mode))
                        .await
                    {
                        Ok(plan) => {
                            self.spawn_peer_message_delivery(plan, reply_tx);
                        }
                        Err(error) => {
                            let _ = reply_tx.send(Err(error));
                        }
                    }
                }
                #[cfg(feature = "runtime-adapter")]
                MobCommand::KickoffOutcomeResolved {
                    agent_identity,
                    outcome,
                    ack_tx,
                } => {
                    if let Err(error) = self.resolve_kickoff_outcome(&agent_identity, outcome).await
                    {
                        tracing::warn!(
                            agent_identity = %agent_identity,
                            error = %error,
                            "failed to persist kickoff outcome"
                        );
                    }
                    let _ = ack_tx.send(());
                }
                MobCommand::RunFlow {
                    flow_id,
                    activation_params,
                    scoped_event_tx,
                    reply_tx,
                } => {
                    let result = self
                        .handle_run_flow(flow_id, activation_params, scoped_event_tx)
                        .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::CancelFlow { run_id, reply_tx } => {
                    let result = self.handle_cancel_flow(run_id).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::FlowStatus { run_id, reply_tx } => {
                    let result = self
                        .run_store
                        .get_run(&run_id)
                        .await
                        .map_err(MobError::from);
                    let _ = reply_tx.send(result);
                }
                MobCommand::CommitFlowRunCommand {
                    run_id,
                    command,
                    context,
                    reply_tx,
                } => {
                    let result = self
                        .commit_flow_run_command_in_actor(&run_id, *command, context)
                        .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::CommitFlowTerminalization {
                    run_id,
                    flow_id,
                    target,
                    command,
                    context,
                    reply_tx,
                } => {
                    let result = self
                        .commit_flow_terminalization_in_actor(
                            run_id, flow_id, target, *command, context,
                        )
                        .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::CommitFlowFrameStorePlan {
                    run_id,
                    plan,
                    reply_tx,
                } => {
                    let result = self
                        .commit_flow_frame_store_plan_in_actor(&run_id, *plan)
                        .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::ProjectMachineInput { input, reply_tx } => {
                    let result = self
                        .apply_dsl_input(*input, "project_machine_input")
                        .map(|()| self.dsl_authority.state().clone());
                    let _ = reply_tx.send(result);
                }
                MobCommand::ApplyMachineInputEffects { input, reply_tx } => {
                    let result =
                        self.apply_dsl_input_collect_effects(*input, "apply_machine_input_effects");
                    let _ = reply_tx.send(result);
                }
                MobCommand::PreviewMachineInput { input, reply_tx } => {
                    let result = self.preview_dsl_input(*input, "preview_machine_input");
                    let _ = reply_tx.send(result);
                }
                MobCommand::QueryMachineState { reply_tx } => {
                    let _ = reply_tx.send(self.dsl_authority.state().clone());
                }
                #[cfg(test)]
                MobCommand::AuthorizeMemberTrustCleanupForTest { edge, reply_tx } => {
                    let result = self
                        .authorize_member_trust_cleanup(
                            &edge,
                            "authorize_member_trust_cleanup_for_test",
                        )
                        .and_then(|handoff| match handoff.authority {
                            MemberTrustAuthority::Unwiring(obligation) => Ok(obligation),
                            MemberTrustAuthority::Wiring(_) | MemberTrustAuthority::Repair(_) => {
                                Err(MobError::WiringError(
                                    "member trust cleanup returned non-unwiring authority"
                                        .to_string(),
                                ))
                            }
                        });
                    let _ = reply_tx.send(result);
                }
                MobCommand::ApplyExternalPeerReciprocalTrust {
                    key,
                    target_comms,
                    peer,
                    reply_tx,
                } => {
                    let result = self
                        .apply_external_peer_reciprocal_trust(key, target_comms, peer)
                        .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::ProjectMachineSignal { signal } => {
                    let foreign_runtime_id =
                        foreign_runtime_observation(self.dsl_authority.state(), &signal).cloned();
                    if let Err(error) = self.apply_dsl_signal(signal, "project_machine_signal") {
                        if let Some(agent_runtime_id) = foreign_runtime_id {
                            tracing::debug!(
                                ?agent_runtime_id,
                                error = %error,
                                "ignored foreign runtime lifecycle observation"
                            );
                        } else {
                            tracing::error!(
                                error = %error,
                                "typed composition signal projection failed"
                            );
                        }
                    }
                }
                MobCommand::FlowFinished { run_id } => {
                    if let Err(error) = self
                        .handle_flow_cleanup(run_id, "flow finished cleanup")
                        .await
                    {
                        tracing::error!(error = %error, "flow finished cleanup failed");
                    }
                }
                MobCommand::FlowCanceledCleanup { run_id } => {
                    if let Err(error) = self
                        .handle_flow_cleanup(run_id, "flow canceled cleanup")
                        .await
                    {
                        tracing::error!(error = %error, "flow canceled cleanup failed");
                    }
                }
                #[cfg(test)]
                MobCommand::FlowTrackerCounts { reply_tx } => {
                    let snapshot = self.flow_tracker_snapshot().await;
                    let tasks = snapshot.run_task_ids.len();
                    let tokens = snapshot.cancel_token_ids.len();
                    let _ = reply_tx.send((tasks, tokens));
                }
                #[cfg(test)]
                MobCommand::OrchestratorSnapshot { reply_tx } => {
                    let phase = self.state();
                    let _ = reply_tx.send(
                        self.machine_orchestrator_snapshot(phase)
                            .unwrap_or_default(),
                    );
                }
                #[cfg(test)]
                MobCommand::LifecycleSnapshot { reply_tx } => {
                    let _ = reply_tx.send(super::state::MobLifecycleSnapshot {
                        phase: self.state(),
                        active_run_count: self.machine_active_run_count(),
                        cleanup_pending: false,
                    });
                }
                #[cfg(test)]
                MobCommand::LifecycleNotificationBurst {
                    count,
                    message,
                    reply_tx,
                } => {
                    for index in 0..count {
                        self.notify_orchestrator_lifecycle(format!("{message} #{index}"))
                            .await;
                    }
                    let _ = reply_tx.send(Ok(()));
                }
                #[cfg(test)]
                MobCommand::DslT2Snapshot { reply_tx } => {
                    let dsl = self.dsl_authority.state();
                    let _ = reply_tx.send(super::state::MobDslT2Snapshot {
                        destroy_admitted: dsl.destroy_admitted,
                        flow_authority_schema_version: dsl.flow_authority_schema_version,
                        owner_bridge_session_id: dsl.owner_bridge_session_id.clone(),
                        owner_bridge_destroy_on_archive: dsl.owner_bridge_destroy_on_archive,
                        implicit_delegation_mob: dsl.implicit_delegation_mob,
                        supervisor_authority_peer_id: dsl.supervisor_authority_peer_id.clone(),
                        supervisor_authority_signing_key: dsl.supervisor_authority_signing_key,
                        supervisor_authority_epoch: dsl.supervisor_authority_epoch,
                        supervisor_authority_protocol_version: dsl
                            .supervisor_authority_protocol_version
                            .clone(),
                        supervisor_pending_authority_peer_id: dsl
                            .supervisor_pending_authority_peer_id
                            .clone(),
                        supervisor_pending_authority_signing_key: dsl
                            .supervisor_pending_authority_signing_key,
                        supervisor_pending_authority_epoch: dsl.supervisor_pending_authority_epoch,
                        supervisor_pending_authority_protocol_version: dsl
                            .supervisor_pending_authority_protocol_version
                            .clone(),
                        supervisor_pending_authority_accepted_peer_ids: dsl
                            .supervisor_pending_authority_accepted_peer_ids
                            .clone(),
                        member_state_markers: dsl.member_state_markers.clone(),
                        wiring_edges: dsl.wiring_edges.clone(),
                        external_peer_edges: dsl.external_peer_edges.clone(),
                        external_peer_edges_by_key: dsl.external_peer_edges_by_key.clone(),
                        identity_to_runtime: dsl.identity_to_runtime.clone(),
                        identity_runtime_generations: dsl.identity_runtime_generations.clone(),
                        identity_runtime_fence_tokens: dsl.identity_runtime_fence_tokens.clone(),
                        member_profile_names: dsl.member_profile_names.clone(),
                        member_runtime_modes: dsl.member_runtime_modes.clone(),
                        member_peer_ids: dsl.member_peer_ids.clone(),
                        member_peer_endpoints: dsl.member_peer_endpoints.clone(),
                        member_restore_failures: dsl.member_restore_failures.clone(),
                        member_session_bindings: dsl.member_session_bindings.clone(),
                        pending_spawn_sessions: dsl.pending_spawn_sessions.clone(),
                        pending_session_ingress_detach_runtime_ids: dsl
                            .pending_session_ingress_detach_runtime_ids
                            .clone(),
                        topology_epoch: dsl.topology_epoch,
                        spawn_policy_enabled: dsl.spawn_policy_enabled,
                        spawn_policy_revision: dsl.spawn_policy_revision,
                        spawn_policy_resolution_revision: dsl
                            .spawn_policy_resolution_revision
                            .clone(),
                        spawn_policy_resolution_profiles: dsl
                            .spawn_policy_resolution_profiles
                            .clone(),
                        spawn_policy_resolution_runtime_modes: dsl
                            .spawn_policy_resolution_runtime_modes
                            .clone(),
                        spawn_policy_resolution_absent: dsl.spawn_policy_resolution_absent.clone(),
                        spawn_profile_authority_profile_names: dsl
                            .spawn_profile_authority_profile_names
                            .clone(),
                        spawn_profile_authority_models: dsl.spawn_profile_authority_models.clone(),
                        spawn_profile_authority_material_digests: dsl
                            .spawn_profile_authority_material_digests
                            .clone(),
                        spawn_profile_authority_tool_config_digests: dsl
                            .spawn_profile_authority_tool_config_digests
                            .clone(),
                        spawn_profile_authority_skills_digests: dsl
                            .spawn_profile_authority_skills_digests
                            .clone(),
                        spawn_profile_authority_provider_params_digests: dsl
                            .spawn_profile_authority_provider_params_digests
                            .clone(),
                        spawn_profile_authority_output_schema_digests: dsl
                            .spawn_profile_authority_output_schema_digests
                            .clone(),
                        spawn_profile_authority_external_addressable: dsl
                            .spawn_profile_authority_external_addressable
                            .clone(),
                    });
                }
                MobCommand::StartupKickoffSnapshot { reply_tx } => {
                    let _ = reply_tx.send(super::state::MobStartupKickoffSnapshot {
                        pending_kickoff_member_ids: self.pending_kickoff_member_ids_from_dsl(),
                        ready_runtime_ids: self.ready_runtime_ids_from_dsl(),
                    });
                }
                MobCommand::ProjectMemberList {
                    include_retiring,
                    reply_tx,
                } => {
                    let _ = reply_tx.send(
                        self.project_member_list_from_machine(include_retiring)
                            .await,
                    );
                }
                MobCommand::ProjectMemberStatus {
                    agent_identity,
                    reply_tx,
                } => {
                    let _ = reply_tx.send(
                        self.machine_member_material(&MeerkatId::from(&agent_identity), true)
                            .await
                            .map(|material| material.to_snapshot()),
                    );
                }
                MobCommand::MemberMachineProjection {
                    agent_identity,
                    reply_tx,
                } => {
                    let _ = reply_tx.send(self.machine_projection_for_identity(&agent_identity));
                }
                MobCommand::Stop { reply_tx } => {
                    let result = if self.state() == MobState::Destroyed {
                        Err(self.invalid_transition_to(MobState::Stopped))
                    } else {
                        match self.probe_command_admission(
                            mob_dsl::MobMachineInput::Stop,
                            MobState::Stopped,
                            "stop_command_admission",
                        ) {
                            Ok(()) => {
                                let mut stop_result =
                                    self.fail_all_pending_spawns("mob is stopping").await;
                                if stop_result.is_ok() {
                                    self.cancel_pending_peer_deliveries("mob is stopping").await;
                                    self.notify_orchestrator_lifecycle(format!(
                                        "Mob '{}' is stopping.",
                                        self.definition.id
                                    ))
                                    .await;
                                    // Cancel checkpointer gates before stopping host loops so
                                    // in-flight saves that complete after the loop stops don't
                                    // race with subsequent external cleanup (e.g. DML deletes).
                                    self.provisioner.cancel_all_checkpointers().await;
                                }
                                if stop_result.is_ok() {
                                    let loop_result = self.stop_all_autonomous_members().await;
                                    if let Err(error) = loop_result {
                                        tracing::warn!(
                                            mob_id = %self.definition.id,
                                            error = %error,
                                            "stop encountered autonomous loop cleanup error"
                                        );
                                        if stop_result.is_ok() {
                                            stop_result = Err(error);
                                        }
                                    }
                                }
                                if stop_result.is_ok() {
                                    if self.has_orchestrator
                                        && let Err(error) = self.apply_dsl_signal(
                                            mob_dsl::MobMachineSignal::StopOrchestrator,
                                            "stop_orchestrator",
                                        )
                                    {
                                        stop_result = Err(MobError::Internal(format!(
                                            "orchestrator StopOrchestrator transition failed during stop: {error}"
                                        )));
                                    }
                                    if stop_result.is_ok()
                                        && let Err(error) = self.apply_command_admission(
                                            mob_dsl::MobMachineInput::Stop,
                                            MobState::Stopped,
                                            "stop_input",
                                        )
                                    {
                                        stop_result = Err(error);
                                    }
                                }
                                if stop_result.is_err() {
                                    self.provisioner.rearm_all_checkpointers().await;
                                }
                                stop_result
                            }
                            Err(error) => Err(error),
                        }
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::ResumeLifecycle { reply_tx } => {
                    let result = match self.probe_command_admission(
                        mob_dsl::MobMachineInput::Resume,
                        MobState::Running,
                        "resume_command_admission",
                    ) {
                        Ok(()) => {
                            // Re-enable checkpointers cancelled during stop.
                            self.provisioner.rearm_all_checkpointers().await;
                            if let Err(error) = self.ensure_autonomous_runtimes_from_roster().await
                            {
                                if let Err(stop_error) = self.stop_all_autonomous_members().await {
                                    tracing::warn!(
                                        mob_id = %self.definition.id,
                                        error = %stop_error,
                                        "resume cleanup failed while stopping autonomous loops"
                                    );
                                }
                                self.provisioner.cancel_all_checkpointers().await;
                                Err(error)
                            } else {
                                let mut resume_result: Result<(), MobError> = Ok(());
                                if self.has_orchestrator
                                    && let Err(error) = self.apply_dsl_signal(
                                        mob_dsl::MobMachineSignal::ResumeOrchestrator,
                                        "resume_orchestrator",
                                    )
                                {
                                    resume_result = Err(MobError::Internal(format!(
                                        "orchestrator ResumeOrchestrator transition failed during resume: {error}"
                                    )));
                                }
                                if resume_result.is_ok()
                                    && let Err(error) = self.apply_command_admission(
                                        mob_dsl::MobMachineInput::Resume,
                                        MobState::Running,
                                        "resume_input",
                                    )
                                {
                                    resume_result = Err(error);
                                }
                                if let Err(error) = resume_result {
                                    if let Err(stop_error) =
                                        self.stop_all_autonomous_members().await
                                    {
                                        tracing::warn!(
                                            mob_id = %self.definition.id,
                                            error = %stop_error,
                                            "resume transition rollback failed while stopping autonomous loops"
                                        );
                                    }
                                    self.provisioner.cancel_all_checkpointers().await;
                                    Err(error)
                                } else {
                                    Ok(())
                                }
                            }
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Complete { reply_tx } => {
                    let result = match self.probe_command_admission(
                        mob_dsl::MobMachineInput::Complete,
                        MobState::Completed,
                        "complete_command_admission",
                    ) {
                        Ok(()) => match self.fail_all_pending_spawns("mob is completing").await {
                            Ok(()) => {
                                self.cancel_pending_peer_deliveries("mob is completing")
                                    .await;
                                self.handle_complete().await
                            }
                            Err(error) => Err(error),
                        },
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Destroy { reply_tx } => {
                    let result = self.handle_destroy().await;
                    let destroy_succeeded = result.is_ok();
                    let _ = reply_tx.send(result);
                    if destroy_succeeded {
                        // Destroy is terminal for the current ownership model:
                        // once a mob is destroyed, the actor task exits and no
                        // further commands are accepted.
                        self.lifecycle_tasks.abort_all();
                        while self.lifecycle_tasks.join_next().await.is_some() {}
                        break;
                    }
                }
                MobCommand::Reset { reply_tx } => {
                    let prior_state = self.state();
                    if let Err(error) = self.probe_command_admission(
                        mob_dsl::MobMachineInput::Reset,
                        MobState::Running,
                        "reset_command_admission",
                    ) {
                        let _ = reply_tx.send(Err(error));
                        continue;
                    }
                    if let Err(error) = self.fail_all_pending_spawns("mob is resetting").await {
                        let _ = reply_tx.send(Err(error));
                        continue;
                    }
                    self.cancel_pending_peer_deliveries("mob is resetting")
                        .await;
                    let result = self.handle_reset(prior_state).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::RotateSupervisor { reply_tx } => {
                    let result = self.handle_rotate_supervisor().await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::PollEvents {
                    after_cursor,
                    limit,
                    reply_tx,
                } => {
                    let result = self
                        .events
                        .poll(after_cursor, limit)
                        .await
                        .map_err(MobError::from);
                    let _ = reply_tx.send(result);
                }
                MobCommand::ReplayAllEvents { reply_tx } => {
                    let result = self.events.replay_all().await.map_err(MobError::from);
                    let _ = reply_tx.send(result);
                }
                MobCommand::RecordOperatorActionProvenance {
                    tool_name,
                    authority_context,
                    reply_tx,
                } => {
                    let result = async {
                        let prepared = self.prepare_dsl_input_transition(
                            mob_dsl::MobMachineInput::RecordOperatorActionProvenance {
                                tool_name,
                                principal_token: authority_context.principal_token().clone(),
                                caller_provenance: authority_context.caller_provenance().cloned(),
                                audit_invocation_id: authority_context
                                    .audit_invocation_id()
                                    .map(ToOwned::to_owned),
                            },
                            "record_operator_action_provenance",
                        )?;
                        let kind = Self::operator_action_recorded_event_from_generated_effect(
                            &prepared.transition,
                            "record_operator_action_provenance",
                        )?;
                        let events = self.events.clone();
                        let mob_id = self.definition.id.clone();
                        self.commit_prepared_dsl_transition_after(prepared, move || async move {
                            events
                                .append(NewMobEvent {
                                    mob_id,
                                    timestamp: None,
                                    kind,
                                })
                                .await
                                .map_err(MobError::from)?;
                            Ok(())
                        })
                        .await?;
                        Ok(())
                    }
                    .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::ForceCancel {
                    agent_identity,
                    reply_tx,
                } => {
                    let result = self.handle_force_cancel(agent_identity).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::Wire {
                    local,
                    target,
                    reply_tx,
                } => {
                    let result = self.handle_wire(local, target).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::WireMembersBatch { edges, reply_tx } => {
                    let result = self.handle_wire_members_batch(edges).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::Unwire {
                    local,
                    target,
                    reply_tx,
                } => {
                    let result = self.handle_unwire(local, target).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::CancelAllWork {
                    runtime_id,
                    fence_token,
                    reply_tx,
                } => {
                    let result = self.handle_cancel_all_work(runtime_id, fence_token).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::SetSpawnPolicy { policy, reply_tx } => {
                    let enabled = policy.is_some();
                    let result = self.apply_dsl_input(
                        mob_dsl::MobMachineInput::SetSpawnPolicy { enabled },
                        "set_spawn_policy",
                    )
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "SetSpawnPolicy rejected by MobMachine guards before shell policy write: {error}"
                        ))
                    });
                    if result.is_ok() {
                        self.spawn_policy.set(policy).await;
                    }
                    let _ = reply_tx.send(result);
                }
                MobCommand::QueryPhase { reply_tx } => {
                    let _ = reply_tx.send(self.state());
                }
                MobCommand::Shutdown { reply_tx } => {
                    if let Err(error) = self.probe_command_admission(
                        mob_dsl::MobMachineInput::Shutdown,
                        MobState::Stopped,
                        "shutdown_command_admission",
                    ) {
                        let _ = reply_tx.send(Err(error));
                        continue;
                    }
                    let mut result = self
                        .fail_all_pending_spawns("mob runtime is shutting down")
                        .await;
                    if result.is_err() {
                        let _ = reply_tx.send(result);
                        continue;
                    }
                    if result.is_ok() {
                        self.cancel_pending_peer_deliveries("mob runtime is shutting down")
                            .await;
                    }
                    if let Err(error) = self.cancel_all_flow_tasks().await {
                        tracing::warn!(error = %error, "shutdown flow cancellation encountered errors");
                        if result.is_ok() {
                            result = Err(error);
                        }
                    }
                    if let Err(error) = self.stop_all_autonomous_members().await {
                        tracing::warn!(error = %error, "shutdown loop stop encountered errors");
                        if result.is_ok() {
                            result = Err(error);
                        }
                    }
                    self.teardown_session_runtime_bindings_from_machine().await;
                    // Cancel remaining lifecycle notification tasks.
                    // abort_all is non-blocking; join_next drains the abort results.
                    self.lifecycle_tasks.abort_all();
                    while self.lifecycle_tasks.join_next().await.is_some() {}
                    if let Err(error) = self.apply_command_admission(
                        mob_dsl::MobMachineInput::Shutdown,
                        MobState::Stopped,
                        "shutdown_input",
                    ) {
                        tracing::warn!(error = %error, "shutdown admission apply failed");
                        if result.is_ok() {
                            result = Err(error);
                        }
                    }
                    let _ = reply_tx.send(result);
                    break;
                }
            }

            // Wave-c C-6p — drain routed-effect queue at every loop
            // boundary. `apply_dsl_input` / `apply_dsl_signal` populated
            // the queue synchronously inside the command handler; this
            // is the async site where `CompositionDispatcher::dispatch`
            // runs. A typed dispatch failure — most notably
            // `DispatchRefusal::UnwiredConsumer` while C-6c (consumer
            // surface on `MeerkatMachine`) is outstanding — is logged
            // loud and the actor task terminates rather than silently
            // dropping the effect. Once the spine lands C-6c, the
            // dispatcher resolves cleanly and this path becomes
            // everyday.
            if self.destroy_admitted()
                && !self.destroy_cleanup_active
                && !self.pending_routed_effects.is_empty()
            {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    queued_remaining = self.pending_routed_effects.len(),
                    "retaining destroy cleanup routed effects for explicit destroy retry"
                );
                continue;
            }
            if let Err(error) = self.flush_routed_effects().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    queued_remaining = self.pending_routed_effects.len(),
                    "composition dispatch failed; terminating mob actor task"
                );
                return;
            }
        }
    }

    fn pending_spawn_cleanup_anchor_for_slot(
        slot: &super::pending_spawn_lineage::PendingSpawnSlot,
        reason: &str,
    ) -> Option<PendingSpawnCleanupAnchor> {
        let snapshot = {
            let progress = slot
                .spawn
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            progress
                .bridge_session_id
                .clone()
                .zip(progress.operation_id.clone())
        };
        snapshot.map(|(session_id, operation_id)| PendingSpawnCleanupAnchor {
            spawn_ticket: slot.ticket,
            agent_identity: slot.spawn.agent_identity.clone(),
            session_id,
            operation_id,
            reason: reason.to_string(),
        })
    }

    async fn cleanup_pending_spawn_anchor(
        &self,
        anchor: &PendingSpawnCleanupAnchor,
    ) -> Result<(), MobError> {
        self.provisioner
            .abort_member_provision(
                &MemberRef::from_bridge_session_id(anchor.session_id.clone()),
                &anchor.operation_id,
                &anchor.reason,
            )
            .await
    }

    fn pending_spawn_cleanup_error(context: &str, errors: Vec<String>) -> MobError {
        MobError::Internal(format!(
            "{context}: pending spawn cleanup incomplete: {}",
            errors.join("; ")
        ))
    }

    async fn drain_pending_spawn_cleanup_anchors(&mut self, context: &str) -> Result<(), MobError> {
        if self.pending_spawn_cleanup_anchors.is_empty() {
            return Ok(());
        }

        let anchors = self
            .pending_spawn_cleanup_anchors
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut errors = Vec::new();
        for anchor in anchors {
            match self.cleanup_pending_spawn_anchor(&anchor).await {
                Ok(()) => {
                    self.pending_spawn_cleanup_anchors
                        .remove(&anchor.spawn_ticket);
                    tracing::info!(
                        spawn_ticket = anchor.spawn_ticket,
                        agent_identity = %anchor.agent_identity,
                        operation_id = %anchor.operation_id,
                        "retried pending spawn cleanup anchor successfully"
                    );
                }
                Err(error) => {
                    errors.push(format!(
                        "{} ticket {} operation {}: {error}",
                        anchor.agent_identity, anchor.spawn_ticket, anchor.operation_id
                    ));
                    tracing::warn!(
                        spawn_ticket = anchor.spawn_ticket,
                        agent_identity = %anchor.agent_identity,
                        operation_id = %anchor.operation_id,
                        error = %error,
                        "pending spawn cleanup anchor still failed"
                    );
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(Self::pending_spawn_cleanup_error(context, errors))
        }
    }

    #[cfg(feature = "runtime-adapter")]
    fn authorize_restored_member_operation_owner(
        &mut self,
        entry: &RosterEntry,
        bridge_session_id: &SessionId,
    ) -> Result<SessionId, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
        let dsl_session_id = mob_dsl::SessionId::from_domain(bridge_session_id);
        let replacing = self
            .dsl_authority
            .state()
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();
        let transition = self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::RecoverMemberSessionBinding {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
                bridge_session_id: dsl_session_id.clone(),
                replacing,
            },
            "restore_member_operation_owner_binding",
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
                "MobMachine restore did not authorize operation owner for '{}'",
                entry.agent_identity
            )));
        }
        Ok(bridge_session_id.clone())
    }

    #[cfg(feature = "runtime-adapter")]
    fn authorize_peer_only_operation_owner_from_machine(
        &mut self,
        agent_identity: &AgentIdentity,
        peer_id: &str,
        context: &'static str,
    ) -> Result<(SessionId, mob_dsl::MemberPeerEndpoint), MobError> {
        let owner_bridge_session_id = self
            .dsl_authority
            .state()
            .owner_bridge_session_id
            .clone()
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "{context}: peer-only operation owner for '{agent_identity}' requires MobMachine owner bridge authority"
                ))
            })?;
        let owner_bridge_session_id =
            SessionId::parse(&owner_bridge_session_id.0).map_err(|error| {
                MobError::Internal(format!(
                    "{context}: MobMachine has invalid owner bridge session for peer-only member '{agent_identity}': {error}"
                ))
            })?;
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let expected_peer_endpoint = self
            .dsl_authority
            .state()
            .member_peer_endpoints
            .get(&dsl_identity)
            .cloned()
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "{context}: peer-only operation owner for '{agent_identity}' requires MobMachine member peer endpoint authority"
                ))
            })?;
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::AuthorizeMemberPeerRebind {
                agent_identity: dsl_identity.clone(),
                expected_peer_endpoint: expected_peer_endpoint.clone(),
            },
            context,
        )?;
        let authorized_peer_endpoint = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::MemberPeerRebindAuthorized {
                    agent_identity: effect_identity,
                    peer_id: effect_peer_id,
                    peer_endpoint,
                } if effect_identity == &dsl_identity
                    && effect_peer_id.0 == peer_id
                    && peer_endpoint == &expected_peer_endpoint =>
                {
                    Some(peer_endpoint.clone())
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "{context}: MobMachine did not authorize peer-only operation owner for '{agent_identity}'"
                ))
            })?;
        Ok((owner_bridge_session_id, authorized_peer_endpoint))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn generated_peer_only_operation_owner_context(
        &mut self,
        agent_identity: &AgentIdentity,
        binding: &crate::RuntimeBinding,
        context: &'static str,
    ) -> Result<
        (
            SessionId,
            Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        ),
        MobError,
    > {
        let crate::RuntimeBinding::External { peer_id, .. } = binding else {
            return Err(MobError::Internal(format!(
                "{context}: peer-only operation owner requested for non-external binding"
            )));
        };
        let observed_peer = Self::peer_only_spec_for_binding(binding, context)?;
        let (owner_bridge_session_id, authorized_endpoint) = self
            .authorize_peer_only_operation_owner_from_machine(agent_identity, peer_id, context)?;
        let authorized_peer =
            Self::peer_only_spec_from_member_endpoint(&authorized_endpoint, context)?;
        if authorized_peer.name != observed_peer.name
            || authorized_peer.peer_id != observed_peer.peer_id
            || authorized_peer.address != observed_peer.address
            || authorized_peer.pubkey != observed_peer.pubkey
        {
            return Err(MobError::Internal(format!(
                "{context}: peer-only operation owner for '{agent_identity}' does not match MobMachine peer endpoint authority"
            )));
        }
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "{context}: peer-only operation owner for '{agent_identity}' requires MeerkatMachine runtime authority"
            ))
        })?;
        let bindings = adapter
            .prepare_local_session_bindings(owner_bridge_session_id.clone())
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "{context}: failed to prepare MeerkatMachine operation bindings for peer-only member '{agent_identity}': {error}"
                ))
            })?;
        if bindings.session_id() != &owner_bridge_session_id {
            return Err(MobError::Internal(format!(
                "{context}: MeerkatMachine operation bindings for peer-only member '{agent_identity}' returned session '{}' but expected '{owner_bridge_session_id}'",
                bindings.session_id()
            )));
        }
        if !meerkat_runtime::session_runtime_bindings_have_machine_authority(&bindings) {
            return Err(MobError::Internal(format!(
                "{context}: MeerkatMachine operation bindings for peer-only member '{agent_identity}' lacked machine authority"
            )));
        }
        Ok((
            owner_bridge_session_id,
            Arc::clone(bindings.ops_lifecycle()),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    fn authorize_restored_peer_only_operation_owner(
        &mut self,
        entry: &RosterEntry,
    ) -> Result<Option<SessionId>, MobError> {
        let MemberRef::BackendPeer {
            peer_id,
            session_id: None,
            ..
        } = &entry.member_ref
        else {
            return Ok(None);
        };
        if self.dsl_authority.state().owner_bridge_session_id.is_none() {
            return Ok(None);
        }
        let (owner_bridge_session_id, _) = self.authorize_peer_only_operation_owner_from_machine(
            &entry.agent_identity,
            peer_id,
            "restore_peer_only_operation_owner_binding",
        )?;
        Ok(Some(owner_bridge_session_id))
    }

    #[cfg(feature = "runtime-adapter")]
    async fn restore_generated_member_operation_bindings(&mut self) -> Result<(), MobError> {
        let Some(adapter) = self.runtime_adapter.clone() else {
            return Ok(());
        };
        let entries = self.roster.read().await.list().cloned().collect::<Vec<_>>();
        for entry in entries {
            let generated_owner_session_id =
                if let Some(bridge_session_id) = entry.member_ref.bridge_session_id().cloned() {
                    self.authorize_restored_member_operation_owner(&entry, &bridge_session_id)?
                } else {
                    let Some(owner_bridge_session_id) =
                        self.authorize_restored_peer_only_operation_owner(&entry)?
                    else {
                        continue;
                    };
                    owner_bridge_session_id
                };
            if matches!(
                entry.member_ref,
                MemberRef::BackendPeer {
                    session_id: None,
                    ..
                }
            ) && !self
                .dsl_authority
                .state()
                .owner_bridge_session_id
                .as_ref()
                .and_then(|session_id| SessionId::parse(&session_id.0).ok())
                .is_some_and(|owner_session_id| owner_session_id == generated_owner_session_id)
            {
                return Err(MobError::Internal(format!(
                    "peer-only operation owner restore for '{}' lost MobMachine owner bridge authority",
                    entry.agent_identity
                )));
            }
            let bindings = adapter
                .prepare_local_session_bindings(generated_owner_session_id.clone())
                .await
                .map_err(|error| {
                    MobError::Internal(format!(
                        "restore operation owner binding failed for member '{}': {error}",
                        entry.agent_identity
                    ))
                })?;
            if bindings.session_id() != &generated_owner_session_id {
                return Err(MobError::Internal(format!(
                    "restore operation owner binding returned session '{}' for member '{}' generated owner '{}'",
                    bindings.session_id(),
                    entry.agent_identity,
                    generated_owner_session_id
                )));
            }
            if !meerkat_runtime::session_runtime_bindings_have_machine_authority(&bindings) {
                return Err(MobError::Internal(format!(
                    "restore operation owner binding lacked MeerkatMachine authority for member '{}'",
                    entry.agent_identity
                )));
            }
            self.provisioner
                .bind_member_owner_context(
                    &entry.member_ref,
                    generated_owner_session_id,
                    Arc::clone(bindings.ops_lifecycle()),
                )
                .await?;
        }
        Ok(())
    }

    #[cfg(not(feature = "runtime-adapter"))]
    async fn restore_generated_member_operation_bindings(&self) -> Result<(), MobError> {
        Ok(())
    }

    async fn fail_all_pending_spawns(&mut self, reason: &str) -> Result<(), MobError> {
        self.drain_pending_spawn_cleanup_anchors(reason).await?;
        if self.pending_spawns.is_empty() {
            if let Some(message) = self.pending_spawn_alignment_violation() {
                tracing::error!(
                    reason,
                    message = %message,
                    "pending spawn alignment violated with no local pending slots to drain"
                );
            }
            return Ok(());
        }

        let mut cleanup_errors = Vec::new();
        for slot in self.pending_spawns.drain_all() {
            let spawn_ticket = slot.ticket;
            let agent_identity = slot.spawn.agent_identity.clone();
            let dsl_identity =
                mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(agent_identity.as_str()));
            if self
                .dsl_authority
                .state()
                .pending_spawn_sessions
                .contains_key(&dsl_identity)
            {
                self.complete_orchestrator_spawn(
                    Some(spawn_ticket),
                    &agent_identity,
                    "lifecycle transition cleared pending spawn",
                );
            }
            if let Err(error) = self.abort_pending_spawn_slot(&slot, reason).await {
                cleanup_errors.push(format!("{agent_identity} ticket {spawn_ticket}: {error}"));
            }
            slot.fail(&format!("spawn canceled for '{agent_identity}': {reason}"));
            tracing::debug!(
                spawn_ticket,
                agent_identity = %agent_identity,
                "failed pending spawn due to lifecycle transition"
            );
        }
        debug_assert!(
            self.pending_spawns.is_empty(),
            "all pending spawn slots should be drained during lifecycle transition"
        );
        if let Some(message) = self.pending_spawn_alignment_violation() {
            tracing::error!(
                message = %message,
                "pending spawn alignment still violated after lifecycle drain"
            );
        }
        if !cleanup_errors.is_empty() {
            return Err(Self::pending_spawn_cleanup_error(reason, cleanup_errors));
        }
        self.drain_pending_spawn_cleanup_anchors(reason).await?;
        Ok(())
    }

    async fn abort_pending_spawn_slot(
        &mut self,
        slot: &super::pending_spawn_lineage::PendingSpawnSlot,
        reason: &str,
    ) -> Result<(), MobError> {
        let Some(anchor) = Self::pending_spawn_cleanup_anchor_for_slot(slot, reason) else {
            return Ok(());
        };
        if let Err(error) = self.cleanup_pending_spawn_anchor(&anchor).await {
            self.pending_spawn_cleanup_anchors
                .insert(anchor.spawn_ticket, anchor.clone());
            tracing::warn!(
                spawn_ticket = anchor.spawn_ticket,
                agent_identity = %anchor.agent_identity,
                operation_id = %anchor.operation_id,
                error = %error,
                "failed to abort pending member provision during lifecycle drain; retained cleanup anchor"
            );
            return Err(MobError::Internal(format!(
                "pending spawn cleanup failed for '{}': {error}",
                anchor.agent_identity
            )));
        }
        self.pending_spawn_cleanup_anchors
            .remove(&anchor.spawn_ticket);
        Ok(())
    }

    async fn cancel_pending_spawns_for_member(
        &mut self,
        agent_identity: &MeerkatId,
        reason: &str,
    ) -> Result<usize, MobError> {
        self.drain_pending_spawn_cleanup_anchors(reason).await?;
        let slots = self.pending_spawns.take_for_member(agent_identity);
        if slots.is_empty() {
            if let Some(message) = self.pending_spawn_alignment_violation() {
                tracing::error!(
                    agent_identity = %agent_identity,
                    reason,
                    message = %message,
                    "pending spawn alignment violated while canceling member-specific pending spawns"
                );
            }
            return Ok(0);
        }
        let canceled = slots.len();

        for slot in &slots {
            self.complete_orchestrator_spawn(
                Some(slot.ticket),
                &slot.spawn.agent_identity,
                "member lifecycle command canceled pending spawn",
            );
        }

        let mut cleanup_errors = Vec::new();
        for slot in slots {
            let spawn_ticket = slot.ticket;
            if let Err(error) = self.abort_pending_spawn_slot(&slot, reason).await {
                cleanup_errors.push(format!("{agent_identity} ticket {spawn_ticket}: {error}"));
            }
            slot.fail(&format!("spawn canceled for '{agent_identity}': {reason}"));
            tracing::debug!(
                spawn_ticket,
                agent_identity = %agent_identity,
                "canceled pending spawn for member lifecycle command"
            );
        }

        self.debug_assert_pending_spawn_alignment();
        if let Some(message) = self.pending_spawn_alignment_violation() {
            tracing::error!(
                agent_identity = %agent_identity,
                message = %message,
                "pending spawn alignment violated after member-specific cancellation"
            );
        }
        if cleanup_errors.is_empty() {
            Ok(canceled)
        } else {
            Err(Self::pending_spawn_cleanup_error(reason, cleanup_errors))
        }
    }

    fn customize_spawn_spec(
        &self,
        spawn_source: super::handle::SpawnSource,
        spawner: Option<&(AgentIdentity, AgentRuntimeId)>,
        spec: &mut super::handle::SpawnMemberSpec,
    ) -> Result<(), MobError> {
        if let Some(customizer) = self.spawn_member_customizer.as_ref() {
            let ctx = super::handle::SpawnCustomizationContext {
                mob_id: self.definition.id.clone(),
                spawn_source,
                spawner_identity: spawner.map(|(identity, _)| identity.clone()),
                spawner_runtime_id: spawner.map(|(_, runtime_id)| runtime_id.clone()),
                requested_profile: spec.role_name.clone(),
            };
            customizer.customize_spawn(&ctx, spec)?;
        }
        Ok(())
    }

    /// P1-T04: spawn() creates a real session.
    ///
    /// Provisioning runs in parallel tasks; final actor commit stays serialized.
    async fn enqueue_spawn(
        &mut self,
        mut spec: super::handle::SpawnMemberSpec,
        spawn_source: super::handle::SpawnSource,
        owner_bridge_session_id: Option<SessionId>,
        ops_registry: Option<Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>>,
        reply_tx: oneshot::Sender<Result<super::handle::MemberSpawnReceipt, MobError>>,
    ) {
        let spawner = match owner_bridge_session_id.as_ref() {
            Some(owner_session_id) => self.spawner_for_bridge_session(owner_session_id).await,
            None => None,
        };
        if let Err(error) = self.customize_spawn_spec(spawn_source, spawner.as_ref(), &mut spec) {
            let _ = reply_tx.send(Err(error));
            return;
        }
        let super::handle::SpawnMemberSpec {
            role_name: profile_name,
            identity,
            initial_message,
            runtime_mode,
            backend,
            binding,
            context,
            labels,
            launch_mode,
            tool_access_policy: _tool_access_policy,
            budget_split_policy: _budget_split_policy,
            auto_wire_parent,
            additional_instructions,
            shell_env,
            inherited_tool_filter,
            override_profile,
            auth_binding,
            external_tools: per_spawn_external_tools,
            system_prompt_override,
            continuity_intent,
        } = spec;
        let agent_identity = MeerkatId::from(identity.as_str());
        if let Err(error) = self.preview_spawn_command_admission(&agent_identity) {
            let _ = reply_tx.send(Err(error));
            return;
        }
        // Normalize launch-mode resume/fork details for the provisioning path.
        let resume_bridge_session_id = launch_mode.resume_bridge_session_id().cloned();
        let fork_spec = match launch_mode {
            crate::launch::MemberLaunchMode::Fork {
                source_member_id,
                fork_context,
            } => Some((source_member_id, fork_context)),
            _ => None,
        };
        let prepare_result = async {
            if agent_identity
                .as_str()
                .starts_with(FLOW_SYSTEM_MEMBER_ID_PREFIX)
            {
                return Err(MobError::WiringError(format!(
                    "meerkat id '{agent_identity}' uses reserved system prefix '{FLOW_SYSTEM_MEMBER_ID_PREFIX}'"
                )));
            }
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                profile = %profile_name,
                "MobActor::enqueue_spawn start"
            );

            if self.pending_spawns.contains_member(&agent_identity) {
                return Err(MobError::MemberAlreadyExists(agent_identity.clone()));
            }

            {
                let roster = self.roster.read().await;
                if roster.get(&agent_identity).is_some() {
                    return Err(MobError::MemberAlreadyExists(agent_identity.clone()));
                }
            }

            // Always validate role_name exists in definition for roster consistency,
            // even when an override profile is provided.
            if !self.definition.profiles.contains_key(&profile_name) {
                return Err(MobError::ProfileNotFound(profile_name.clone()));
            }

            // Capture the override for roster persistence before consuming it.
            let effective_profile_override = override_profile.clone();

            // Use override_profile if provided (from SpawnTooling::Profile resolution),
            // otherwise resolve from the mob definition.
            let mut profile = if let Some(p) = override_profile {
                p
            } else {
                self.definition
                    .resolve_profile(&profile_name, self.realm_profile_store.as_ref())
                    .await?
            };
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                profile = %profile_name,
                "MobActor::enqueue_spawn profile resolved"
            );
            if inherited_tool_filter.is_some() && effective_profile_override.is_none() {
                build::open_profile_tool_categories_for_inherited_filter(&mut profile);
            }
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                profile = %profile_name,
                "MobActor::enqueue_spawn authorizing profile material"
            );
            let authorized_profile_material = self.authorize_spawn_profile_material(
                &agent_identity,
                &profile_name,
                &profile,
                "enqueue_spawn_profile_authority",
            )?;
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                profile = %profile_name,
                "MobActor::enqueue_spawn profile material authorized"
            );

            let selected_runtime_mode = runtime_mode.unwrap_or(profile.runtime_mode);
            let profile_external_addressable = authorized_profile_material.external_addressable;
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                profile = %profile_name,
                "MobActor::enqueue_spawn resolving external tools"
            );

            // ---------- Resume bridge-session fast-path ----------
            // When resume_bridge_session_id is set, skip provisioning and go
            // straight to finalization. The bridge session must already exist
            // and be usable.
            if let Some(resume_id) = resume_bridge_session_id {
                let member_ref = MemberRef::from_bridge_session_id(resume_id.clone());

                // Validate the session exists and is active.
                let is_active = self
                    .provisioner
                    .is_member_active(&member_ref)
                    .await
                    .map_err(|e| {
                        MobError::Internal(format!(
                            "resume bridge session check failed for '{agent_identity}': {e}"
                        ))
                    })?;
                if is_active.unwrap_or(false) {
                    // Validate interaction-scoped injection for autonomous mode.
                    if selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost
                        && self.provisioner.interaction_event_injector(&resume_id).await.is_none()
                    {
                        return Err(MobError::MissingMemberCapability {
                            member_id: agent_identity.clone(),
                            capability: crate::error::MobMemberCapability::InteractionEventInjector,
                            context: "autonomous member resume",
                        });
                    }

                    // Validate comms if wiring rules exist.
                    let has_wiring = self.definition.wiring.auto_wire_orchestrator
                        || !self.definition.wiring.role_wiring.is_empty();
                    if has_wiring
                        && self
                            .provisioner
                            .comms_runtime(&member_ref)
                            .await
                            .is_none()
                    {
                        return Err(MobError::Internal(format!(
                            "resumed session '{resume_id}' has no comms runtime for '{agent_identity}'"
                        )));
                    }

                    let prompt = initial_message.clone().unwrap_or_else(|| {
                        ContentInput::from(self.fallback_spawn_prompt(&profile_name, &agent_identity))
                    });
                    let initial_turn_prompt = initial_message.as_ref().map(|_| prompt.clone());
                    let resolved_labels = labels.unwrap_or_default();

                    return Ok((
                        profile_name,
                        agent_identity,
                        prompt,
                        initial_turn_prompt,
                        selected_runtime_mode,
                        profile_external_addressable,
                        resolved_labels,
                        Some(member_ref),
                        None,
                        owner_bridge_session_id.clone(),
                        auto_wire_parent,
                        effective_profile_override.clone(),
                        authorized_profile_material.clone(),
                        continuity_intent.clone(),
                    ));
                }

                if self.session_service.supports_persistent_sessions() {
                    let stored_session = self
                        .session_service
                        .load_persisted_session(&resume_id)
                        .await
                        .map_err(MobError::from)?
                        .ok_or_else(|| {
                            MobError::Internal(format!(
                                "missing durable session snapshot for '{resume_id}'"
                            ))
                        })?;

                    let external_tools =
                        self.external_tools_for_profile(&profile, per_spawn_external_tools.clone())?;
                    let mut config = build::build_resumed_agent_config(
                        build::BuildResumedAgentConfigParams {
                            base: build::BuildAgentConfigParams {
                                mob_id: &self.definition.id,
                                profile_name: &profile_name,
                                agent_identity: &agent_identity,
                                profile: &profile,
                                definition: &self.definition,
                                external_tools,
                                context,
                                labels: labels.clone(),
                                additional_instructions,
                                shell_env,
                                mob_tool_authority_context: None,
                                inherited_tool_filter: inherited_tool_filter.clone(),
                                system_prompt_override: system_prompt_override.clone(),
                            },
                            expected_session_id: &resume_id,
                            resumed_session: stored_session,
                        },
                    )
                    .await?;
                    config.keep_alive =
                        selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost;
                    if let Some(ref client) = self.default_llm_client {
                        config.llm_client_override = Some(client.clone());
                    }

                    let prompt = initial_message.clone().unwrap_or_else(|| {
                        ContentInput::from(self.fallback_spawn_prompt(&profile_name, &agent_identity))
                    });
                    let initial_turn_prompt = initial_message.as_ref().map(|_| prompt.clone());
                    let req = build::to_create_session_request(&config, prompt.clone());
                    let selected_binding = resolve_binding(
                        binding.clone(),
                        backend,
                        profile.backend,
                        self.definition.backend.default,
                        &agent_identity,
                    )?;
                    let selected_runtime_mode =
                        normalize_runtime_mode_for_binding(selected_runtime_mode, &selected_binding);
                    let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, agent_identity);
                    let provision_request = ProvisionMemberRequest {
                        create_session: req,
                        binding: selected_binding,
                        peer_name,
                        owner_bridge_session_id: owner_bridge_session_id.clone(),
                        ops_registry: ops_registry.clone(),
                        generated_self_owned_operation_owner: None,
                    };
                    let resolved_labels = labels.unwrap_or_default();
                    return Ok((
                        profile_name,
                        agent_identity,
                        prompt,
                        initial_turn_prompt,
                        selected_runtime_mode,
                        profile_external_addressable,
                        resolved_labels,
                        None::<MemberRef>,
                        Some(provision_request),
                        owner_bridge_session_id.clone(),
                        auto_wire_parent,
                        effective_profile_override.clone(),
                        authorized_profile_material.clone(),
                        continuity_intent.clone(),
                    ));
                }

                return Err(MobError::Internal(format!(
                    "resumed session '{resume_id}' not found or inactive for '{agent_identity}'"
                )));
            }

            // ---------- Fork path ----------
            // When fork_spec is set, read source member's session and render
            // conversation history as context in the initial prompt.
            let fork_context_text = if let Some((source_member_id, fork_context)) = fork_spec {
                let source_session_id = {
                    let roster = self.roster.read().await;
                    let source_entry = roster.get(&source_member_id).ok_or_else(|| {
                        MobError::MemberNotFound(source_member_id.clone())
                    })?;
                    source_entry
                        .member_ref
                        .bridge_session_id()
                        .cloned()
                        .ok_or_else(|| {
                            MobError::Internal(format!(
                                "fork source '{source_member_id}' has no session"
                            ))
                        })?
                };

                // Read full history for fork context rendering
                let query = match fork_context {
                    crate::launch::ForkContext::FullHistory => {
                        meerkat_core::service::SessionHistoryQuery::default()
                    }
                    crate::launch::ForkContext::LastMessages { count } => {
                        // We need last N messages; read_history uses offset/limit.
                        // First read the session to get message count.
                        let view = self
                            .session_service
                            .read(&source_session_id)
                            .await
                            .map_err(|e| {
                                MobError::Internal(format!(
                                    "failed to read source session metadata for fork from '{source_member_id}': {e}"
                                ))
                            })?;
                        let total = view.state.message_count;
                        let offset = total.saturating_sub(count as usize);
                        meerkat_core::service::SessionHistoryQuery {
                            offset,
                            limit: Some(count as usize),
                        }
                    }
                };

                let history = meerkat_core::service::SessionServiceHistoryExt::read_history(
                    self.session_service.as_ref(),
                    &source_session_id,
                    query,
                )
                .await
                .map_err(|e| {
                    MobError::Internal(format!(
                        "failed to read source session history for fork from '{source_member_id}': {e}"
                    ))
                })?;

                Some(render_fork_context(&source_member_id, &history.messages))
            } else {
                None
            };

            let external_tools =
                self.external_tools_for_profile(&profile, per_spawn_external_tools.clone())?;
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                profile = %profile_name,
                "MobActor::enqueue_spawn external tools resolved"
            );
            let mut config = build::build_agent_config(build::BuildAgentConfigParams {
                mob_id: &self.definition.id,
                profile_name: &profile_name,
                agent_identity: &agent_identity,
                profile: &profile,
                definition: &self.definition,
                external_tools,
                context,
                labels: labels.clone(),
                additional_instructions,
                shell_env,
                mob_tool_authority_context: None,
                inherited_tool_filter,
                system_prompt_override,
            })
            .await?;
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                profile = %profile_name,
                "MobActor::enqueue_spawn agent config built"
            );
            config.keep_alive =
                selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost;
            if let Some(ref client) = self.default_llm_client {
                config.llm_client_override = Some(client.clone());
            }
            // Deferral §1: per-member auth binding.
            if let Some(ref cref) = auth_binding {
                config.auth_binding = Some(cref.clone());
            }

            let base_prompt = initial_message.clone().unwrap_or_else(|| {
                ContentInput::from(self.fallback_spawn_prompt(&profile_name, &agent_identity))
            });
            let prompt = if let Some(fork_text) = fork_context_text {
                let mut blocks = vec![meerkat_core::types::ContentBlock::Text {
                    text: format!("{fork_text}\n\n"),
                }];
                blocks.extend(base_prompt.into_blocks());
                ContentInput::Blocks(blocks)
            } else {
                base_prompt
            };
            let initial_turn_prompt = initial_message.as_ref().map(|_| prompt.clone());
            let req = build::to_create_session_request(&config, prompt.clone());
            let selected_binding = resolve_binding(
                binding,
                backend,
                profile.backend,
                self.definition.backend.default,
                &agent_identity,
            )?;
            let selected_runtime_mode =
                normalize_runtime_mode_for_binding(selected_runtime_mode, &selected_binding);
            let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, agent_identity);
            let provision_request = ProvisionMemberRequest {
                create_session: req,
                binding: selected_binding,
                peer_name,
                owner_bridge_session_id: owner_bridge_session_id.clone(),
                ops_registry: ops_registry.clone(),
                generated_self_owned_operation_owner: None,
            };
            let resolved_labels = labels.unwrap_or_default();
            Ok((
                profile_name,
                agent_identity,
                prompt,
                initial_turn_prompt,
                selected_runtime_mode,
                profile_external_addressable,
                resolved_labels,
                None::<MemberRef>,
                Some(provision_request),
                owner_bridge_session_id.clone(),
                auto_wire_parent,
                effective_profile_override,
                authorized_profile_material,
                continuity_intent,
            ))
        }
        .await;

        let (
            profile_name,
            agent_identity,
            prompt,
            initial_turn_prompt,
            selected_runtime_mode,
            _external_addressable,
            resolved_labels,
            resume_member_ref,
            maybe_provision_request,
            spawn_owner_bridge_session_id,
            auto_wire_parent,
            effective_profile_override,
            authorized_profile_material,
            continuity_intent,
        ) = match prepare_result {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = reply_tx.send(Err(error));
                return;
            }
        };

        // ---------- Resume fast-path: skip async provisioning ----------
        if let Some(member_ref) = resume_member_ref {
            let Some(bridge_session_id) = member_ref.bridge_session_id().cloned() else {
                let _ = reply_tx.send(Err(MobError::Internal(format!(
                    "resumed member '{agent_identity}' has no bridge session id"
                ))));
                return;
            };
            if let Err(error) = self.preview_spawn_admission(
                &agent_identity,
                &authorized_profile_material,
                Some(&bridge_session_id),
            ) {
                let _ = reply_tx.send(Err(error));
                return;
            }
            if let (Some(owner_bridge_session_id), Some(ops_registry)) =
                (owner_bridge_session_id.clone(), ops_registry.clone())
                && let Err(error) = self
                    .provisioner
                    .bind_member_owner_context(&member_ref, owner_bridge_session_id, ops_registry)
                    .await
            {
                let _ = reply_tx.send(Err(error));
                return;
            }
            let operation_id = self
                .provisioner
                .active_operation_id_for_member(&member_ref)
                .await
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "resumed member '{agent_identity}' has no tracked mob child operation"
                    ))
                });
            let operation_id = match operation_id {
                Ok(operation_id) => operation_id,
                Err(error) => {
                    let _ = reply_tx.send(Err(error));
                    return;
                }
            };
            let provision =
                PendingProvision::new(member_ref, agent_identity.clone(), self.provisioner.clone());
            // Go straight to finalization — no async provisioning task needed.
            let fence = self.issue_fence_token();
            let result = self
                .finalize_spawn_from_pending(
                    &profile_name,
                    &agent_identity,
                    crate::ids::Generation::INITIAL,
                    fence,
                    selected_runtime_mode,
                    prompt,
                    initial_turn_prompt,
                    resolved_labels,
                    provision,
                    operation_id,
                    spawn_owner_bridge_session_id,
                    auto_wire_parent,
                    None,
                    effective_profile_override,
                    authorized_profile_material,
                    continuity_intent,
                )
                .await
                .map(|outcome| outcome.receipt);
            let _ = reply_tx.send(result);
            return;
        }

        // Normal provisioning path — resume path already returned above.
        let Some(mut provision_request) = maybe_provision_request else {
            let _ = reply_tx.send(Err(MobError::Internal(
                "provision_request missing for normal spawn path".into(),
            )));
            return;
        };
        let admitted_bridge_session_id =
            admit_bridge_session_for_spawn(&mut provision_request.create_session);
        let spawn_bridge_session_id = match &provision_request.binding {
            crate::RuntimeBinding::Session => Some(&admitted_bridge_session_id),
            crate::RuntimeBinding::External { .. } => None,
        };

        if let Err(error) = self.preview_spawn_admission(
            &agent_identity,
            &authorized_profile_material,
            spawn_bridge_session_id,
        ) {
            let _ = reply_tx.send(Err(error));
            return;
        }

        let spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        let spawn_meerkat_id = agent_identity.clone();
        let spawn_meerkat_id_for_log = spawn_meerkat_id.clone();
        let spawn_runtime_mode = selected_runtime_mode;
        let pending_progress = Arc::new(std::sync::Mutex::new(PendingSpawnProgress::default()));

        let generated_self_owned_operation_owner =
            match self.stage_orchestrator_spawn(&agent_identity, &admitted_bridge_session_id) {
                Ok(authority) => authority,
                Err(error) => {
                    let _ = reply_tx.send(Err(error));
                    return;
                }
            };
        if let Err(error) = Self::apply_generated_self_owned_operation_owner(
            &mut provision_request,
            generated_self_owned_operation_owner,
        ) {
            let _ = reply_tx.send(Err(error));
            return;
        }

        let pending = PendingSpawn {
            profile_name,
            agent_identity,
            admitted_bridge_session_id,
            prompt,
            initial_turn_prompt,
            runtime_mode: selected_runtime_mode,
            labels: resolved_labels,
            owner_bridge_session_id: spawn_owner_bridge_session_id,
            auto_wire_parent,
            restore_wiring: None,
            effective_profile_override,
            authorized_profile_material,
            continuity_intent,
            progress: pending_progress.clone(),
            reply_tx,
        };
        // Treat pending spawn lifecycle as a single keyed table: pending intent
        // and async task handle must be inserted/removed together.
        let provisioner = self.provisioner.clone();
        let command_tx = self.command_tx.clone();
        let task = tokio::spawn(async move {
            let panic_meerkat_id = spawn_meerkat_id.clone();
            let provision_result = std::panic::AssertUnwindSafe(async {
                let spawn_receipt = provisioner.provision_member(provision_request).await?;
                if let Some(bridge_session_id) =
                    spawn_receipt.member_ref.bridge_session_id().cloned()
                {
                    let mut progress = pending_progress
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    progress.bridge_session_id = Some(bridge_session_id);
                    progress.operation_id = Some(spawn_receipt.operation_id.clone());
                }
                #[cfg(test)]
                {
                    let delay_ms = SPAWN_PROVISIONED_COMMAND_DELAY_MS
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }
                if spawn_runtime_mode == crate::MobRuntimeMode::AutonomousHost
                    && let Err(capability_error) =
                        Self::ensure_autonomous_dispatch_capability_for_provisioner(
                            &provisioner,
                            &spawn_meerkat_id,
                            &spawn_receipt.member_ref,
                        )
                        .await
                {
                    if let Err(retire_error) =
                        provisioner.retire_member(&spawn_receipt.member_ref).await
                    {
                        return Err(MobError::Internal(format!(
                            "autonomous capability check failed for '{spawn_meerkat_id}': {capability_error}; cleanup retire failed for member '{:?}': {retire_error}",
                            spawn_receipt.member_ref
                        )));
                    }
                    return Err(capability_error);
                }
                Ok(spawn_receipt)
            })
            .catch_unwind()
            .await;
            let provision_result = match provision_result {
                Ok(result) => result,
                Err(_) => Err(MobError::Internal(format!(
                    "spawn provisioning task panicked for '{panic_meerkat_id}'"
                ))),
            };

            if let Err(send_error) = command_tx
                .send(MobCommand::SpawnProvisioned {
                    spawn_ticket,
                    result: provision_result,
                })
                .await
                && let MobCommand::SpawnProvisioned {
                    result: Ok(spawn_receipt),
                    ..
                } = send_error.0
                && let Err(cleanup_error) =
                    provisioner.retire_member(&spawn_receipt.member_ref).await
            {
                tracing::warn!(
                    spawn_ticket,
                    member_ref = ?spawn_receipt.member_ref,
                    error = %cleanup_error,
                    "spawn completion dropped; failed cleanup retire for provisioned member"
                );
            }
        });
        self.insert_pending_spawn(spawn_ticket, pending, task);
        if let Err(error) = self.ensure_pending_spawn_alignment("enqueue_spawn post-insert") {
            tracing::error!(
                spawn_ticket,
                error = %error,
                "pending spawn alignment check failed after enqueue; canceling all pending spawns"
            );
            if let Err(cleanup_error) = self
                .fail_all_pending_spawns("pending spawn alignment violated after enqueue")
                .await
            {
                tracing::error!(
                    spawn_ticket,
                    error = %cleanup_error,
                    "pending spawn cleanup failed after enqueue alignment violation"
                );
            }
            return;
        }

        tracing::debug!(
            spawn_ticket,
            agent_identity = %spawn_meerkat_id_for_log,
            runtime_mode = ?spawn_runtime_mode,
            "MobActor::enqueue_spawn queued provisioning task"
        );
    }

    async fn handle_spawn_provisioned_batch(
        &mut self,
        completions: Vec<(u64, Result<super::handle::MemberSpawnReceipt, MobError>)>,
    ) {
        tracing::debug!(
            completion_count = completions.len(),
            "MobActor::handle_spawn_provisioned_batch start"
        );
        if let Err(error) = self.ensure_pending_spawn_alignment("spawn batch preflight") {
            tracing::error!(
                error = %error,
                "pending spawn alignment check failed before spawn completion batch"
            );
            if let Err(cleanup_error) = self
                .fail_all_pending_spawns("pending spawn alignment violated before spawn batch")
                .await
            {
                tracing::error!(
                    error = %cleanup_error,
                    "pending spawn cleanup failed after pre-batch alignment violation"
                );
            }
            return;
        }

        let mut pending_items = Vec::with_capacity(completions.len());
        for (spawn_ticket, result) in completions {
            tracing::debug!(
                spawn_ticket,
                "MobActor::handle_spawn_provisioned_batch completing pending slot"
            );
            let (pending, task_handle) =
                self.complete_pending_spawn_slot(spawn_ticket, "spawn provisioned batch");
            let Some(pending) = pending else {
                tracing::warn!(spawn_ticket, "received spawn completion for unknown ticket");
                if let Some(handle) = task_handle {
                    handle.abort();
                    tracing::warn!(
                        spawn_ticket,
                        "received spawn completion for unknown pending metadata but found task handle"
                    );
                }
                if let Ok(spawn_receipt) = result {
                    let orphan = PendingProvision::new(
                        spawn_receipt.member_ref,
                        MeerkatId::from("__unknown_ticket__"),
                        self.provisioner.clone(),
                    );
                    if let Err(error) = orphan.rollback().await {
                        tracing::warn!(
                            spawn_ticket,
                            error = %error,
                            "unknown spawn completion cleanup failed"
                        );
                    }
                }
                continue;
            };
            pending_items.push((pending, result));
        }

        for (pending, result) in pending_items {
            tracing::debug!("MobActor::handle_spawn_provisioned_batch finalizing pending spawn");
            let PendingSpawn {
                profile_name,
                agent_identity,
                admitted_bridge_session_id: _,
                prompt,
                initial_turn_prompt,
                runtime_mode,
                labels,
                owner_bridge_session_id,
                auto_wire_parent,
                restore_wiring,
                effective_profile_override,
                authorized_profile_material,
                continuity_intent,
                progress: _,
                reply_tx,
            } = pending;
            let reply = match result {
                Ok(spawn_receipt) => {
                    let provision = PendingProvision::new(
                        spawn_receipt.member_ref.clone(),
                        agent_identity.clone(),
                        self.provisioner.clone(),
                    );
                    if let Err(error) = self.require_state(&[MobState::Running]) {
                        if let Err(retire_error) = provision.rollback().await {
                            Err(MobError::Internal(format!(
                                "spawn completed while mob state changed for '{agent_identity}': {error}; cleanup retire failed: {retire_error}"
                            )))
                        } else {
                            Err(error)
                        }
                    } else {
                        let fence = self.issue_fence_token();
                        tracing::debug!(
                            agent_identity = %agent_identity,
                            "MobActor::handle_spawn_provisioned_batch calling finalize_spawn_from_pending"
                        );
                        self.finalize_spawn_from_pending(
                            &profile_name,
                            &agent_identity,
                            crate::ids::Generation::INITIAL,
                            fence,
                            runtime_mode,
                            prompt,
                            initial_turn_prompt,
                            labels,
                            provision,
                            spawn_receipt.operation_id,
                            owner_bridge_session_id,
                            auto_wire_parent,
                            restore_wiring,
                            effective_profile_override,
                            authorized_profile_material,
                            continuity_intent,
                        )
                        .await
                        .map(|outcome| outcome.receipt)
                    }
                }
                Err(error) => Err(error),
            };
            let _ = reply_tx.send(reply);
        }

        if let Err(error) = self.ensure_pending_spawn_alignment("spawn batch completion") {
            tracing::error!(
                error = %error,
                "pending spawn alignment check failed after spawn completion batch"
            );
            if let Err(cleanup_error) = self
                .fail_all_pending_spawns("pending spawn alignment violated after spawn batch")
                .await
            {
                tracing::error!(
                    error = %cleanup_error,
                    "pending spawn cleanup failed after spawn batch alignment violation"
                );
            }
        }
    }

    async fn spawn_from_policy_inline(
        &mut self,
        agent_identity: &MeerkatId,
        spawn_spec: super::spawn_policy::SpawnSpec,
        work_ref: &WorkRef,
        origin: WorkOrigin,
    ) -> Result<super::handle::MemberSpawnReceipt, MobError> {
        self.ensure_pending_spawn_alignment("spawn_from_policy_inline preflight")?;

        let requested_identity = AgentIdentity::from(agent_identity.as_str());
        let mut member_spec =
            super::handle::SpawnMemberSpec::new(spawn_spec.profile, requested_identity.clone());
        member_spec.runtime_mode = spawn_spec.runtime_mode;
        // Policy auto-spawn material is the MobMachine-recorded
        // SpawnPolicyResolutionRecorded effect. Build-boundary customizers are
        // intentionally skipped here so shell code cannot mutate the
        // post-authority profile/runtime/capability material before
        // provisioning.

        let super::handle::SpawnMemberSpec {
            role_name: profile_name,
            identity: _,
            initial_message,
            runtime_mode,
            backend,
            binding,
            context,
            labels,
            launch_mode: _,
            tool_access_policy: _tool_access_policy,
            budget_split_policy: _budget_split_policy,
            auto_wire_parent: _,
            additional_instructions,
            shell_env,
            inherited_tool_filter,
            override_profile,
            auth_binding,
            external_tools: per_spawn_external_tools,
            system_prompt_override,
            continuity_intent,
        } = member_spec;

        if agent_identity
            .as_str()
            .starts_with(FLOW_SYSTEM_MEMBER_ID_PREFIX)
        {
            return Err(MobError::WiringError(format!(
                "meerkat id '{agent_identity}' uses reserved system prefix '{FLOW_SYSTEM_MEMBER_ID_PREFIX}'"
            )));
        }
        if self.pending_spawns.contains_member(agent_identity) {
            return Err(MobError::MemberAlreadyExists(agent_identity.clone()));
        }
        {
            let roster = self.roster.read().await;
            if roster.get(agent_identity).is_some() {
                return Err(MobError::MemberAlreadyExists(agent_identity.clone()));
            }
        }

        let mut profile = if let Some(p) = override_profile.clone() {
            p
        } else {
            self.definition
                .resolve_profile(&profile_name, self.realm_profile_store.as_ref())
                .await?
        };
        if inherited_tool_filter.is_some() && override_profile.is_none() {
            build::open_profile_tool_categories_for_inherited_filter(&mut profile);
        }
        let authorized_profile_material = self.authorize_spawn_profile_material(
            agent_identity,
            &profile_name,
            &profile,
            "policy_spawn_profile_authority",
        )?;
        self.preview_policy_spawn_submit_work_admission(
            &requested_identity,
            &authorized_profile_material,
            work_ref,
            origin,
        )?;
        let runtime_mode = runtime_mode.unwrap_or(profile.runtime_mode);
        let selected_binding = resolve_binding(
            binding,
            backend,
            profile.backend,
            self.definition.backend.default,
            agent_identity,
        )?;
        let runtime_mode = normalize_runtime_mode_for_binding(runtime_mode, &selected_binding);
        let external_tools = self.external_tools_for_profile(&profile, per_spawn_external_tools)?;
        let labels = labels.unwrap_or_default();
        let mut config = build::build_agent_config(build::BuildAgentConfigParams {
            mob_id: &self.definition.id,
            profile_name: &profile_name,
            agent_identity,
            profile: &profile,
            definition: &self.definition,
            external_tools,
            context,
            labels: Some(labels.clone()),
            additional_instructions,
            shell_env,
            mob_tool_authority_context: None,
            inherited_tool_filter,
            system_prompt_override,
        })
        .await?;
        config.keep_alive = runtime_mode == crate::MobRuntimeMode::AutonomousHost;
        if let Some(ref client) = self.default_llm_client {
            config.llm_client_override = Some(client.clone());
        }
        if let Some(ref cref) = auth_binding {
            config.auth_binding = Some(cref.clone());
        }

        let prompt = initial_message.clone().unwrap_or_else(|| {
            ContentInput::from(self.fallback_spawn_prompt(&profile_name, agent_identity))
        });
        let initial_turn_prompt = initial_message.as_ref().map(|_| prompt.clone());
        let req = build::to_create_session_request(&config, prompt.clone());
        let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, agent_identity);
        let mut provision_request = ProvisionMemberRequest {
            create_session: req,
            binding: selected_binding,
            peer_name,
            owner_bridge_session_id: None,
            ops_registry: None,
            generated_self_owned_operation_owner: None,
        };
        let admitted_bridge_session_id =
            admit_bridge_session_for_spawn(&mut provision_request.create_session);

        let spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        let generated_self_owned_operation_owner =
            self.stage_orchestrator_spawn(agent_identity, &admitted_bridge_session_id)?;
        Self::apply_generated_self_owned_operation_owner(
            &mut provision_request,
            generated_self_owned_operation_owner,
        )?;
        let (pending_reply_tx, _pending_reply_rx) = oneshot::channel();
        let pending = PendingSpawn {
            profile_name: profile_name.clone(),
            agent_identity: agent_identity.clone(),
            admitted_bridge_session_id,
            prompt: prompt.clone(),
            initial_turn_prompt: initial_turn_prompt.clone(),
            runtime_mode,
            labels: labels.clone(),
            owner_bridge_session_id: None,
            auto_wire_parent: false,
            restore_wiring: None,
            effective_profile_override: override_profile.clone(),
            authorized_profile_material: authorized_profile_material.clone(),
            continuity_intent: continuity_intent.clone(),
            progress: Arc::new(std::sync::Mutex::new(PendingSpawnProgress::default())),
            reply_tx: pending_reply_tx,
        };
        let pending_task = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        self.insert_pending_spawn(spawn_ticket, pending, pending_task);
        if let Err(error) =
            self.ensure_pending_spawn_alignment("spawn_from_policy_inline staged pending")
        {
            tracing::error!(
                agent_identity = %agent_identity,
                error = %error,
                "pending spawn alignment violated while staging inline policy spawn"
            );
            self.fail_all_pending_spawns(
                "pending spawn alignment violated while staging inline policy spawn",
            )
            .await?;
            return Err(error);
        }

        let spawn_result = Box::pin(async {
            let spawn_receipt = self.provisioner.provision_member(provision_request).await?;
            if runtime_mode == crate::MobRuntimeMode::AutonomousHost
                && let Err(capability_error) =
                    Self::ensure_autonomous_dispatch_capability_for_provisioner(
                        &self.provisioner,
                        agent_identity,
                        &spawn_receipt.member_ref,
                    )
                    .await
            {
                if let Err(retire_error) = self
                    .provisioner
                    .retire_member(&spawn_receipt.member_ref)
                    .await
                {
                    return Err(MobError::Internal(format!(
                        "autonomous capability check failed for '{agent_identity}': {capability_error}; cleanup retire failed: {retire_error}"
                    )));
                }
                return Err(capability_error);
            }
            let provision = PendingProvision::new(
                spawn_receipt.member_ref.clone(),
                agent_identity.clone(),
                self.provisioner.clone(),
            );
            if let Err(error) = self.require_state(&[MobState::Running]) {
                if let Err(retire_error) = provision.rollback().await {
                    return Err(MobError::Internal(format!(
                        "policy spawn completed while mob state changed for '{agent_identity}': {error}; cleanup retire failed: {retire_error}"
                    )));
                }
                return Err(error);
            }
            let fence = self.issue_fence_token();
            self.finalize_spawn_from_pending(
                &profile_name,
                agent_identity,
                crate::ids::Generation::INITIAL,
                fence,
                runtime_mode,
                prompt,
                initial_turn_prompt,
                labels,
                provision,
                spawn_receipt.operation_id,
                None,
                false,
                None,
                override_profile, // policy spawns usually use definition profiles; customizers may supply an override
                authorized_profile_material,
                continuity_intent,
            )
            .await
            .map(|outcome| outcome.receipt)
        })
        .await;

        let (_pending, task_handle) =
            self.complete_pending_spawn_slot(spawn_ticket, "policy inline spawn completion");
        if let Some(handle) = task_handle {
            handle.abort();
        }
        if let Err(error) =
            self.ensure_pending_spawn_alignment("spawn_from_policy_inline completion")
        {
            tracing::error!(
                agent_identity = %agent_identity,
                error = %error,
                "pending spawn alignment violated after inline policy spawn completion"
            );
            self.fail_all_pending_spawns(
                "pending spawn alignment violated after inline policy spawn completion",
            )
            .await?;
            return Err(error);
        }

        spawn_result
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize_spawn_from_pending(
        &mut self,
        profile_name: &ProfileName,
        agent_identity: &MeerkatId,
        generation: crate::ids::Generation,
        fence_token: crate::ids::FenceToken,
        runtime_mode: crate::MobRuntimeMode,
        prompt: ContentInput,
        initial_turn_prompt: Option<ContentInput>,
        labels: std::collections::BTreeMap<String, String>,
        provision: PendingProvision,
        operation_id: meerkat_core::ops::OperationId,
        owner_bridge_session_id: Option<SessionId>,
        auto_wire_parent: bool,
        restore_wiring: Option<RestoreWiringPlan>,
        effective_profile_override: Option<crate::profile::Profile>,
        authorized_profile_material: AuthorizedSpawnProfileMaterial,
        continuity_intent: super::handle::SpawnContinuityIntent,
    ) -> Result<FinalizeSpawnOutcome, MobError> {
        tracing::debug!(
            agent_identity = %agent_identity,
            profile = %profile_name,
            runtime_mode = ?runtime_mode,
            "MobActor::finalize_spawn_from_pending start"
        );
        let identity = crate::ids::AgentIdentity::from(agent_identity.as_str());
        let agent_runtime_id = crate::ids::AgentRuntimeId::new(identity.clone(), generation);
        let overlay_record =
            self.external_binding_overlay_record(&identity, generation, provision.member_ref());
        let external_addressable = authorized_profile_material.external_addressable;

        // Feed `Spawn` into the MobMachine DSL so it populates
        // `live_runtime_ids` + `externally_addressable_runtime_ids` and
        // downstream guards (Retire, SubmitWork, …) operate on authoritative
        // membership state.
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);
        let bridge_session_id = provision
            .member_ref()
            .bridge_session_id()
            .map(mob_dsl::SessionId::from_domain);
        let replacing = self
            .dsl_authority
            .state()
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending preparing DSL Spawn"
        );
        let prepared_spawn = self.prepare_dsl_input_transition(
            mob_dsl::MobMachineInput::Spawn {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&agent_runtime_id),
                fence_token: mob_dsl::FenceToken::from_domain(fence_token),
                generation: mob_dsl::Generation::from_domain(generation),
                profile_material_digest: authorized_profile_material.profile_material_digest,
                external_addressable,
                runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::from(runtime_mode),
                bridge_session_id: bridge_session_id.clone(),
                replacing,
            },
            "finalize_spawn_from_pending_dsl_spawn",
        )?;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending prepared DSL Spawn"
        );
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending validating lifecycle journal"
        );
        Self::require_member_lifecycle_journal_effect(
            &prepared_spawn.transition,
            mob_dsl::MobLifecycleJournalKind::MemberSpawned,
            &identity,
            &agent_runtime_id,
            Some(fence_token),
            generation,
            bridge_session_id.clone(),
            "finalize_spawn_from_pending_dsl_spawn",
        )?;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending validated lifecycle journal"
        );

        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending resolving supervisor comms"
        );
        let supervisor_private_trust_install = if let (Some(session_id), Some(comms)) = (
            provision.member_ref().bridge_session_id().cloned(),
            self.provisioner_comms(provision.member_ref()).await,
        ) {
            tracing::debug!(
                agent_identity = %agent_identity,
                session_id = %session_id,
                "MobActor::finalize_spawn_from_pending installing supervisor private trust"
            );
            match self
                .install_supervisor_private_trust_for_session(&session_id, &comms, None)
                .await
            {
                Ok(install) => {
                    tracing::debug!(
                        agent_identity = %agent_identity,
                        session_id = %session_id,
                        "MobActor::finalize_spawn_from_pending installed supervisor private trust"
                    );
                    Some((session_id, comms, install))
                }
                Err(error) => {
                    if let Err(rollback_error) = provision.rollback().await {
                        return Err(MobError::Internal(format!(
                            "spawn supervisor private trust failed for '{agent_identity}': {error}; archive compensation failed: {rollback_error}"
                        )));
                    }
                    return Err(error);
                }
            }
        } else {
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::finalize_spawn_from_pending skipped supervisor private trust"
            );
            None
        };

        if let Some(overlay_record) = overlay_record.as_ref() {
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::finalize_spawn_from_pending upserting overlay"
            );
            if let Err(error) = self
                .runtime_metadata
                .upsert_external_binding_overlay(&self.definition.id, overlay_record)
                .await
            {
                if let Some((session_id, comms, install)) =
                    supervisor_private_trust_install.as_ref()
                {
                    self.cleanup_supervisor_private_trust_for_session(session_id, comms, install)
                        .await;
                }
                if let Err(rollback_error) = provision.rollback().await {
                    return Err(MobError::Internal(format!(
                        "spawn overlay upsert failed for '{agent_identity}': {error}; archive compensation failed: {rollback_error}"
                    )));
                }
                return Err(error.into());
            }
        }
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending appending spawn event"
        );
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberSpawned({
                    let mut event = crate::event::MemberSpawnedEvent::new(
                        identity.clone(),
                        generation,
                        fence_token,
                        agent_runtime_id.clone(),
                        profile_name.clone(),
                    )
                    .with_bridge_member_ref(Some(Self::sanitized_member_ref(
                        provision.member_ref(),
                    )));
                    event.runtime_mode = runtime_mode;
                    event.labels = labels.clone();
                    event.continuity_intent = continuity_intent.clone();
                    event
                }),
            })
            .await
        {
            if overlay_record.is_some() {
                let _ = self
                    .delete_external_binding_overlay_for_member(&identity, generation)
                    .await;
            }
            if let Some((session_id, comms, install)) = supervisor_private_trust_install.as_ref() {
                self.cleanup_supervisor_private_trust_for_session(session_id, comms, install)
                    .await;
            }
            if let Err(rollback_error) = provision.rollback().await {
                return Err(MobError::Internal(format!(
                    "spawn append failed for '{agent_identity}': {append_error}; archive compensation failed: {rollback_error}"
                )));
            }
            return Err(MobError::from(append_error));
        }
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending committing DSL spawn"
        );
        self.commit_prepared_dsl_transition(prepared_spawn)?;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending committed DSL spawn"
        );

        // Commit the provision: the member is now owned by the roster.
        // From this point, rollback_failed_spawn handles cleanup via the
        // disposal pipeline.
        let member_ref = provision.commit()?;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending committed provision"
        );
        self.restore_diagnostics
            .write()
            .await
            .remove(agent_identity);
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending cleared diagnostics"
        );

        // Populate the Roster projection AFTER DSL `Spawn` authoritatively
        // applies. The pre-DSL roster insert was deleted in Wave-A commit
        // `e77ce8797` (running before `MobMachineInput::Spawn` committed, so
        // rejected admissions could leave shell state stale); the
        // correctly-ordered replacement was never wired until now. Without
        // this insert, `start_autonomous_member` below reads an empty roster
        // and fails with `"autonomous member '{id}' missing roster entry for
        // startup readiness"` (#30 D-spawn-readiness-lookup).
        //
        // `peer_id` is the canonical comms routing UUID. The MobMachine also
        // records the full descriptor so generated member trust authority is
        // bound to the exact name/address/signing key that will be installed.
        let (peer_descriptor, transport_public_key) =
            if let Some(session_id) = member_ref.bridge_session_id() {
                match self.session_service.comms_runtime(session_id).await {
                    Some(runtime) => {
                        let public_key_bytes = runtime.public_key_bytes();
                        let descriptor = match public_key_bytes {
                            Some(_) => {
                                let comms_name =
                                    format!("{}/{}/{}", self.definition.id, profile_name, identity);
                                Some(
                                    self.provisioner
                                        .trusted_peer_spec_for_operation(
                                            &member_ref,
                                            &operation_id,
                                            &comms_name,
                                            "",
                                        )
                                        .await?,
                                )
                            }
                            None => None,
                        };
                        let public_key = public_key_bytes
                            .map(|pubkey| meerkat_comms::PubKey::new(pubkey).to_pubkey_string());
                        (descriptor, public_key)
                    }
                    None => (None, None),
                }
            } else if let MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                ..
            } = &member_ref
            {
                let descriptor =
                    Self::peer_only_spec_from_parts(peer_id, address, "finalize_spawn", *pubkey)?;
                (Some(descriptor), None)
            } else {
                (None, None)
            };
        let peer_id = peer_descriptor
            .as_ref()
            .map(|descriptor| descriptor.peer_id);
        if let Some(descriptor) = peer_descriptor.as_ref() {
            self.apply_dsl_input(
                mob_dsl::MobMachineInput::RegisterMemberPeer {
                    agent_identity: dsl_identity.clone(),
                    peer_endpoint: mob_dsl::MemberPeerEndpoint::from(descriptor),
                },
                "finalize_spawn_register_member_peer",
            )?;
        }
        {
            let mut roster = self.roster.write().await;
            roster.add_member(crate::roster::RosterAddEntry {
                agent_identity: identity.clone(),
                generation,
                fence_token,
                agent_runtime_id: agent_runtime_id.clone(),
                role: profile_name.clone(),
                runtime_mode,
                member_ref: Self::sanitized_member_ref(&member_ref),
                peer_id,
                transport_public_key,
                labels: labels.clone(),
                effective_profile_override: effective_profile_override.clone(),
            });
        }

        if runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            let _ = self
                .apply_kickoff_input(
                    agent_identity,
                    mob_dsl::MobMachineInput::KickoffMarkPending {
                        member_id: agent_identity.to_string(),
                    },
                )
                .await?;
        }
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending roster updated"
        );

        // Wave-A damage restored: `spawn_wiring_targets` computes the
        // auto-wire + role-wiring fan-out targets for this spawn, and the
        // imperative wire-call loop deleted alongside the pre-DSL
        // `roster.add_member` insert (commit `e77ce8797`) needs to run here
        // so role-wired profiles actually get wired at spawn time. The
        // compensating rollback in `rollback_failed_spawn` expects both
        // `wired_spawn_targets` and `planned_wiring_targets` populated so
        // partial-wire failures can unwind.
        let mut planned_wiring_targets = self
            .spawn_wiring_targets(profile_name, agent_identity)
            .await;
        if auto_wire_parent
            && let Some(parent_target) = self
                .resolve_auto_wire_parent_target(owner_bridge_session_id.as_ref(), agent_identity)
                .await
            && !planned_wiring_targets.contains(&parent_target)
        {
            planned_wiring_targets.push(parent_target);
        }
        let mut wired_spawn_targets: Vec<MeerkatId> = Vec::new();
        for target in &planned_wiring_targets {
            let target_identity = crate::ids::AgentIdentity::from(target.as_str());
            let local_meerkat = agent_identity.clone();
            match self
                .handle_wire(
                    local_meerkat,
                    super::handle::PeerTarget::Local(target_identity),
                )
                .await
            {
                Ok(()) => wired_spawn_targets.push(target.clone()),
                Err(wire_error) => {
                    let surfaced_wire_error = match wire_error {
                        MobError::WiringError(_) => wire_error,
                        other => MobError::WiringError(other.to_string()),
                    };
                    // Rollback the spawn: the member is in the DSL + roster
                    // but the role-wiring contract was violated. Surface the
                    // failure to the caller so they can decide how to
                    // compensate (tests assert this path at e.g.
                    // `test_role_wiring_failure_is_returned_to_spawn_caller`).
                    self.clear_kickoff_state(agent_identity).await;
                    if let Err(rollback_error) = self
                        .rollback_failed_spawn(
                            agent_identity,
                            profile_name,
                            &member_ref,
                            &wired_spawn_targets,
                            &planned_wiring_targets,
                        )
                        .await
                    {
                        return Err(MobError::Internal(format!(
                            "spawn wire fan-out failed for '{agent_identity}': {surfaced_wire_error}; rollback failed: {rollback_error}"
                        )));
                    }
                    return Err(surfaced_wire_error);
                }
            }
        }

        #[cfg(feature = "runtime-adapter")]
        if runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            let _ = self
                .apply_kickoff_input(
                    agent_identity,
                    mob_dsl::MobMachineInput::KickoffMarkStarting {
                        member_id: agent_identity.to_string(),
                    },
                )
                .await?;
            // Spawn emits RequestRuntimeBinding. Drain it before startup can
            // publish RuntimeBound, otherwise the session may emit a fallback
            // runtime id that MobMachine correctly rejects as not live.
            if let Err(binding_error) = self.flush_routed_effects().await {
                self.clear_kickoff_state(agent_identity).await;
                if let Err(rollback_error) = self
                    .rollback_failed_spawn(
                        agent_identity,
                        profile_name,
                        &member_ref,
                        &wired_spawn_targets,
                        &planned_wiring_targets,
                    )
                    .await
                {
                    return Err(MobError::Internal(format!(
                        "spawn runtime binding failed for '{agent_identity}': {binding_error}; rollback failed: {rollback_error}"
                    )));
                }
                return Err(binding_error);
            }
            if let Err(start_error) = self
                .start_autonomous_member(agent_identity, &member_ref, prompt)
                .await
            {
                self.clear_kickoff_state(agent_identity).await;
                if let Err(rollback_error) = self
                    .rollback_failed_spawn(
                        agent_identity,
                        profile_name,
                        &member_ref,
                        &wired_spawn_targets,
                        &planned_wiring_targets,
                    )
                    .await
                {
                    return Err(MobError::Internal(format!(
                        "spawn host-loop start failed for '{agent_identity}': {start_error}; rollback failed: {rollback_error}"
                    )));
                }
                return Err(start_error);
            }
        }

        if runtime_mode == crate::MobRuntimeMode::TurnDriven {
            // Turn-driven mob members still need a persistent comms drain:
            // async peer requests/responses arrive between user turns (think
            // realtime audio operators calling `send_request` and waiting for
            // `send_response`). Without a drain running, the
            // `peer_response_terminal` notice never reaches the session's
            // runtime queue, so the wake path is dead. The drain-spawn seam
            // is independent of `config.keep_alive` (which the mock session
            // services overload as "block on start_turn"), so we drive it
            // explicitly here for turn-driven members that have a bridge
            // session and a comms runtime.
            #[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
            if let (Some(adapter), Some(bridge_session_id)) =
                (self.runtime_adapter.clone(), member_ref.bridge_session_id())
            {
                let comms_runtime = self.provisioner.comms_runtime(&member_ref).await;
                if std::env::var_os("RKAT_TRACE_COMMS_DRAIN_BIND").is_some()
                    && let Some(runtime) = comms_runtime.as_ref()
                {
                    tracing::info!(
                        agent_identity = %agent_identity,
                        session_id = %bridge_session_id,
                        comms_ptr = ?Arc::as_ptr(runtime),
                        "mob turn-driven spawn binding comms drain"
                    );
                }
                // W2-G: route through the mob-owned spawn seam so peer-ingress
                // ownership transitions to `MobOwned { comms_runtime_id, mob_id }`.
                if let Some(comms_runtime) = comms_runtime {
                    let mob_id = meerkat_runtime::meerkat_machine::dsl::MobId::from(
                        self.definition.id.as_ref(),
                    );
                    let _ = adapter
                        .maybe_spawn_mob_comms_drain(bridge_session_id, comms_runtime, mob_id)
                        .await;
                }
            }

            if let Some(initial_turn_prompt) = initial_turn_prompt {
                if let Err(start_error) = self
                    .dispatch_turn_driven_spawn_initial_turn(
                        agent_identity,
                        &agent_runtime_id,
                        fence_token,
                        &operation_id,
                        initial_turn_prompt,
                    )
                    .await
                {
                    if let Err(rollback_error) = self
                        .rollback_failed_spawn(
                            agent_identity,
                            profile_name,
                            &member_ref,
                            &wired_spawn_targets,
                            &planned_wiring_targets,
                        )
                        .await
                    {
                        return Err(MobError::Internal(format!(
                            "turn-driven spawn initial turn failed for '{agent_identity}': {start_error}; rollback failed: {rollback_error}"
                        )));
                    }
                    return Err(start_error);
                }
            }
        }

        // Respawn restore: re-fire topology captured from MobMachine, not
        // from roster projection fields. These loop results are shell
        // observations only; `ResolveRespawnTopologyRestore` below owns the
        // public respawn result class.
        let mut failed_restore_peer_ids: Vec<RespawnTopologyPeerId> = Vec::new();
        if let Some(plan) = restore_wiring {
            for peer_identity in plan.local_peers {
                if peer_identity == *agent_identity {
                    continue;
                }
                let peer_agent_identity = crate::ids::AgentIdentity::from(peer_identity.as_str());
                if let Err(error) = self
                    .handle_wire(
                        agent_identity.clone(),
                        super::handle::PeerTarget::Local(peer_agent_identity),
                    )
                    .await
                {
                    tracing::warn!(
                        agent_identity = %agent_identity,
                        peer = %peer_identity,
                        %error,
                        "respawn: failed to restore machine-owned local peer edge"
                    );
                    failed_restore_peer_ids
                        .push(RespawnTopologyPeerId::from(peer_identity.as_str()));
                }
            }
            for peer_spec in plan.external_peers {
                let peer_id = RespawnTopologyPeerId::from(peer_spec.peer_id.as_str());
                if let Err(error) = self
                    .handle_wire(
                        agent_identity.clone(),
                        super::handle::PeerTarget::External(peer_spec.clone()),
                    )
                    .await
                {
                    tracing::warn!(
                        agent_identity = %agent_identity,
                        peer = %peer_spec.name,
                        %error,
                        "respawn: failed to restore machine-owned external peer edge"
                    );
                    failed_restore_peer_ids.push(peer_id);
                }
            }
        }

        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending done"
        );
        Ok(FinalizeSpawnOutcome {
            receipt: super::handle::MemberSpawnReceipt {
                member_ref,
                operation_id,
            },
            failed_restore_peer_ids,
        })
    }

    async fn spawn_wiring_targets(
        &self,
        profile_name: &ProfileName,
        agent_identity: &MeerkatId,
    ) -> Vec<MeerkatId> {
        let mut targets = Vec::new();

        if self.definition.wiring.auto_wire_orchestrator
            && let Some(orchestrator) = &self.definition.orchestrator
            && profile_name != &orchestrator.profile
        {
            let orchestrator_ids =
                self.active_machine_member_ids_for_profile(&orchestrator.profile, agent_identity);
            for orchestrator_id in orchestrator_ids {
                if orchestrator_id != *agent_identity && !targets.contains(&orchestrator_id) {
                    targets.push(orchestrator_id);
                }
            }
        }

        for rule in &self.definition.wiring.role_wiring {
            let target_profile = if &rule.a == profile_name {
                Some(&rule.b)
            } else if &rule.b == profile_name {
                Some(&rule.a)
            } else {
                None
            };
            if let Some(target_profile) = target_profile {
                let target_ids =
                    self.active_machine_member_ids_for_profile(target_profile, agent_identity);
                for target_id in target_ids {
                    if !targets.contains(&target_id) {
                        targets.push(target_id);
                    }
                }
            }
        }

        targets
    }

    async fn resolve_auto_wire_parent_target(
        &self,
        owner_bridge_session_id: Option<&SessionId>,
        spawned_meerkat_id: &MeerkatId,
    ) -> Option<MeerkatId> {
        let owner_bridge_session_id = owner_bridge_session_id?;
        let dsl_session_id = mob_dsl::SessionId::from_domain(owner_bridge_session_id);
        let dsl = self.dsl_authority.state();
        dsl.member_session_bindings
            .iter()
            .find(|(identity, bound_session_id)| {
                **bound_session_id == dsl_session_id
                    && identity.0.as_str() != spawned_meerkat_id.as_str()
                    && !dsl.member_restore_failures.contains_key(*identity)
                    && MobMemberLifecycleProjection::is_active_machine_lifecycle(
                        &dsl.member_lifecycle_for_identity(identity),
                    )
            })
            .map(|(identity, _)| MeerkatId::from(identity.0.as_str()))
    }

    async fn spawner_for_bridge_session(
        &self,
        owner_bridge_session_id: &SessionId,
    ) -> Option<(AgentIdentity, AgentRuntimeId)> {
        let dsl_session_id = mob_dsl::SessionId::from_domain(owner_bridge_session_id);
        let dsl = self.dsl_authority.state();
        dsl.member_session_bindings
            .iter()
            .find_map(|(identity, bound_session_id)| {
                if *bound_session_id != dsl_session_id
                    || !MobMemberLifecycleProjection::is_active_machine_lifecycle(
                        &dsl.member_lifecycle_for_identity(identity),
                    )
                {
                    return None;
                }
                let agent_identity = AgentIdentity::from(identity.0.as_str());
                let (agent_runtime_id, _) = dsl
                    .member_runtime_material_for_identity(identity)?
                    .to_domain_for_identity(&agent_identity);
                Some((agent_identity, agent_runtime_id))
            })
    }

    /// P1-T05: force-cancel a member's in-flight turn cooperatively.
    ///
    /// Does NOT retire the member — the member remains in the roster and can
    /// receive new turns. Use [`handle_retire`] to fully remove a member.
    async fn handle_force_cancel(&mut self, agent_identity: MeerkatId) -> Result<(), MobError> {
        let prepared = self.prepare_command_admission(
            mob_dsl::MobMachineInput::ForceCancel {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(
                    agent_identity.as_str(),
                )),
            },
            MobState::Running,
            "force_cancel",
        )?;
        let member_ref = {
            let roster = self.roster.read().await;
            roster
                .get(&agent_identity)
                .map(|entry| entry.member_ref.clone())
        };

        if let Some(member_ref) = member_ref {
            self.provisioner.interrupt_member(&member_ref).await?;
        } else {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                "MobMachine admitted force-cancel without a roster projection; committing machine authority without mechanical interrupt"
            );
        }
        self.commit_prepared_dsl_input(prepared)?;
        Ok(())
    }

    /// D-wire-handler (#26) + #31 D-trust-reconciliation (Wave D): forward a
    /// wire command to the MobMachine DSL and install bidirectional comms
    /// trust + peer notifications.
    ///
    /// Shell-mechanical steps on a successful local-local wire:
    ///
    /// 1. Normalize `(local, target)` into a canonical `WiringEdge`.
    /// 2. Resolve both endpoints' comms runtimes + trusted-peer specs. Any
    ///    missing comms runtime / public key fails fast as
    ///    [`MobError::WiringError`] with **zero side effects**.
    /// 3. Submit `MobMachineInput::WireMembers { edge }` to the DSL
    ///    authority. Already-wired idempotency is a generated no-op
    ///    transition; only `WiringGraphChanged` means the machine graph
    ///    actually mutated.
    /// 4. On DSL acceptance, bidirectionally install trust on both
    ///    runtimes (A trusts B, B trusts A) and emit `mob.peer_added`
    ///    notifications from both sides. Any failure mid-step rolls back
    ///    the trust installs and reverts the DSL wire so failure leaves
    ///    no observable side effect. This replaces the pre-DSL
    ///    trust-install loop that Wave-A commit `0ad584cde` deleted.
    /// 5. Append `MembersWired` event and project it through the roster.
    ///    Append failure also rolls back trust + DSL wire.
    ///
    /// External peer targets are routed to [`Self::handle_wire_external`],
    /// whose descriptor-bearing trust edge is admitted by
    /// `MobMachineInput::WireExternalPeer` instead of being coerced into a
    /// member `WiringEdge`. Public surfaces should pass
    /// [`PeerTarget::ExternalBinding`], which this actor resolves before trust
    /// installation; pre-resolved [`PeerTarget::External`] is retained for
    /// internal callers and tests.
    async fn handle_wire(
        &mut self,
        local: MeerkatId,
        target: super::handle::PeerTarget,
    ) -> Result<(), MobError> {
        let peer_identity = match target {
            super::handle::PeerTarget::Local(id) => id,
            super::handle::PeerTarget::ExternalName(name) => {
                return Err(MobError::WiringError(format!(
                    "wire external peer '{name}' requires external_binding"
                )));
            }
            super::handle::PeerTarget::ExternalBinding(binding) => {
                let descriptor = Self::trusted_peer_descriptor_from_external_binding(binding)?;
                return self.handle_wire_external(local, descriptor).await;
            }
            super::handle::PeerTarget::External(descriptor) => {
                return self.handle_wire_external(local, descriptor).await;
            }
        };

        let local_identity = AgentIdentity::from(local.as_str());
        let peer_meerkat_id = MeerkatId::from(peer_identity.as_str());
        let dsl_a = mob_dsl::AgentIdentity::from_domain(&local_identity);
        let dsl_b = mob_dsl::AgentIdentity::from_domain(&peer_identity);
        let edge = mob_dsl::WiringEdge::new(dsl_a, dsl_b);
        self.probe_command_admission(
            mob_dsl::MobMachineInput::WireMembers { edge: edge.clone() },
            MobState::Running,
            "wire_members_command_admission",
        )?;

        if local_identity == peer_identity {
            return Err(MobError::WiringError(format!(
                "wire requires distinct members (got '{local}')"
            )));
        }
        self.ensure_member_not_broken(&local).await?;

        self.ensure_member_not_broken(&peer_meerkat_id).await?;

        // Pre-flight: roster lookups. Missing members fail fast before
        // any authority mutation.
        let (local_entry, peer_entry) = {
            let roster = self.roster.read().await;
            (
                roster
                    .get(&local)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(local.clone()))?,
                roster
                    .get(&peer_meerkat_id)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(peer_meerkat_id.clone()))?,
            )
        };

        // Idempotent repair path when the MobMachine already owns the edge.
        // The DSL emits a local repair effect; without a graph-change effect,
        // this path may reinstall live trust but must not synthesize a public
        // roster/event projection.
        let dsl_has_edge = self
            .dsl_authority
            .state()
            .wiring_edges
            .iter()
            .any(|existing| existing == &edge);

        // Resolve both endpoints' comms runtimes + specs BEFORE mutating
        // any authority state. Missing comms / missing public key yields
        // WiringError with zero side effects.
        let local_endpoint = self.resolve_wiring_endpoint(&local_entry, "wire").await?;
        let peer_endpoint = self.resolve_wiring_endpoint(&peer_entry, "wire").await?;
        if dsl_has_edge {
            let authority = self.apply_wire_members_idempotent(&edge)?;
            if !authority.is_repair() {
                return Err(MobError::WiringError(
                    "idempotent wire repair did not produce generated repair authority".to_string(),
                ));
            }
            let handoff = authority.member_handoff()?;
            match (&local_endpoint, &peer_endpoint) {
                (
                    WiringEndpoint::Local {
                        comms: local_comms,
                        spec: local_spec,
                        ..
                    },
                    WiringEndpoint::Local {
                        comms: peer_comms,
                        spec: peer_spec,
                        ..
                    },
                ) => {
                    let peer_key = Self::trusted_peer_removal_key(peer_spec);
                    let local_key = Self::trusted_peer_removal_key(local_spec);
                    handoff.require_peer_id_for(&peer_meerkat_id, &peer_key)?;
                    let local_trust_created = match self
                        .apply_trusted_peer_add_report(
                            local_comms.as_ref(),
                            peer_spec.clone(),
                            handoff.repair_authority_for(
                                &peer_meerkat_id,
                                &peer_key,
                                &self.dsl_authority,
                            )?,
                        )
                        .await
                    {
                        Ok(created) => created,
                        Err(error) => return Err(MobError::from(error)),
                    };
                    handoff.require_peer_id_for(&local, &local_key)?;
                    if let Err(error) = self
                        .apply_trusted_peer_add_report(
                            peer_comms.as_ref(),
                            local_spec.clone(),
                            handoff.repair_authority_for(
                                &local,
                                &local_key,
                                &self.dsl_authority,
                            )?,
                        )
                        .await
                    {
                        if local_trust_created {
                            let rollback_handoff = self.authorize_member_trust_unwiring(
                                &edge,
                                "wire_members_repair_rollback_trust_authority",
                            )?;
                            let _ = self
                                .apply_trusted_peer_remove(
                                    local_comms.as_ref(),
                                    peer_key.clone(),
                                    rollback_handoff
                                        .unwiring_authority_for(&peer_meerkat_id, &peer_key)?,
                                )
                                .await;
                        }
                        return Err(MobError::from(error));
                    }
                }
                (
                    WiringEndpoint::PeerOnly {
                        spec: local_spec,
                        binding: local_binding,
                    },
                    WiringEndpoint::PeerOnly {
                        spec: peer_spec,
                        binding: peer_binding,
                    },
                ) => {
                    self.wire_peer_only_recipient(
                        local_spec,
                        Some(local_binding),
                        peer_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await
                    .map_err(|error| {
                        tracing::debug!(
                            mob_id = %self.definition.id,
                            %error,
                            "peer-only trust repair failed before reciprocal side"
                        );
                        error
                    })?;
                    if let Err(error) = self
                        .wire_peer_only_recipient(
                            peer_spec,
                            Some(peer_binding),
                            local_spec,
                            std::time::Duration::from_secs(10),
                        )
                        .await
                    {
                        self.rollback_peer_only_wire(
                            &edge,
                            false,
                            &["local"],
                            &local,
                            &peer_meerkat_id,
                            local_spec,
                            peer_spec,
                        )
                        .await;
                        return Err(error);
                    }
                }
                (
                    WiringEndpoint::Local {
                        comms: local_comms,
                        spec: local_spec,
                        ..
                    },
                    WiringEndpoint::PeerOnly {
                        spec: peer_spec,
                        binding: peer_binding,
                    },
                ) => {
                    let peer_key = Self::trusted_peer_removal_key(peer_spec);
                    handoff.require_peer_id_for(&peer_meerkat_id, &peer_key)?;
                    let local_trust_created = match self
                        .apply_trusted_peer_add_report(
                            local_comms.as_ref(),
                            peer_spec.clone(),
                            handoff.repair_authority_for(
                                &peer_meerkat_id,
                                &peer_key,
                                &self.dsl_authority,
                            )?,
                        )
                        .await
                    {
                        Ok(created) => created,
                        Err(error) => return Err(MobError::from(error)),
                    };
                    if let Err(error) = self
                        .wire_peer_only_recipient(
                            peer_spec,
                            Some(peer_binding),
                            local_spec,
                            std::time::Duration::from_secs(10),
                        )
                        .await
                    {
                        if local_trust_created {
                            let rollback_handoff = self.authorize_member_trust_unwiring(
                                &edge,
                                "wire_members_peer_only_repair_rollback_trust_authority",
                            )?;
                            let _ = self
                                .apply_trusted_peer_remove(
                                    local_comms.as_ref(),
                                    peer_key.clone(),
                                    rollback_handoff
                                        .unwiring_authority_for(&peer_meerkat_id, &peer_key)?,
                                )
                                .await;
                        }
                        return Err(error);
                    }
                }
                (
                    WiringEndpoint::PeerOnly {
                        spec: local_spec,
                        binding: local_binding,
                    },
                    WiringEndpoint::Local {
                        comms: peer_comms,
                        spec: peer_spec,
                        ..
                    },
                ) => {
                    let local_key = Self::trusted_peer_removal_key(local_spec);
                    handoff.require_peer_id_for(&local, &local_key)?;
                    let peer_trust_created = match self
                        .apply_trusted_peer_add_report(
                            peer_comms.as_ref(),
                            local_spec.clone(),
                            handoff.repair_authority_for(
                                &local,
                                &local_key,
                                &self.dsl_authority,
                            )?,
                        )
                        .await
                    {
                        Ok(created) => created,
                        Err(error) => return Err(MobError::from(error)),
                    };
                    if let Err(error) = self
                        .wire_peer_only_recipient(
                            local_spec,
                            Some(local_binding),
                            peer_spec,
                            std::time::Duration::from_secs(10),
                        )
                        .await
                    {
                        if peer_trust_created {
                            let rollback_handoff = self.authorize_member_trust_unwiring(
                                &edge,
                                "wire_members_peer_only_repair_rollback_trust_authority",
                            )?;
                            let _ = self
                                .apply_trusted_peer_remove(
                                    peer_comms.as_ref(),
                                    local_key.clone(),
                                    rollback_handoff.unwiring_authority_for(&local, &local_key)?,
                                )
                                .await;
                        }
                        return Err(error);
                    }
                }
            }
            return Ok(());
        }
        if let (
            WiringEndpoint::PeerOnly {
                spec: local_spec,
                binding: local_binding,
            },
            WiringEndpoint::PeerOnly {
                spec: peer_spec,
                binding: peer_binding,
            },
        ) = (&local_endpoint, &peer_endpoint)
        {
            let authority = self.apply_wire_members_idempotent(&edge)?;
            let dsl_added = authority.dsl_added();
            if let Err(error) = self
                .wire_peer_only_recipient(
                    local_spec,
                    Some(local_binding),
                    peer_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                self.rollback_peer_only_wire(
                    &edge,
                    dsl_added,
                    &[],
                    &local,
                    &peer_meerkat_id,
                    local_spec,
                    peer_spec,
                )
                .await;
                return Err(error);
            }
            if let Err(error) = self
                .wire_peer_only_recipient(
                    peer_spec,
                    Some(peer_binding),
                    local_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                self.rollback_peer_only_wire(
                    &edge,
                    dsl_added,
                    &["local"],
                    &local,
                    &peer_meerkat_id,
                    local_spec,
                    peer_spec,
                )
                .await;
                return Err(error);
            }
            let event = NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MembersWired {
                    a: AgentIdentity::from(edge.a.0.as_str()),
                    b: AgentIdentity::from(edge.b.0.as_str()),
                },
            };
            let stored = match self.events.append(event).await {
                Ok(stored) => stored,
                Err(error) => {
                    self.rollback_peer_only_wire(
                        &edge,
                        dsl_added,
                        &["local", "peer"],
                        &local,
                        &peer_meerkat_id,
                        local_spec,
                        peer_spec,
                    )
                    .await;
                    return Err(MobError::from(error));
                }
            };
            self.roster.write().await.apply_event(&stored);
            return Ok(());
        }
        if let (
            WiringEndpoint::Local {
                comms: local_comms,
                spec: local_spec,
                ..
            },
            WiringEndpoint::PeerOnly {
                spec: peer_spec,
                binding: peer_binding,
            },
        ) = (&local_endpoint, &peer_endpoint)
        {
            let authority = self.apply_wire_members_idempotent(&edge)?;
            let dsl_added = authority.dsl_added();
            let handoff = authority.member_handoff()?;
            let peer_key = Self::trusted_peer_removal_key(peer_spec);
            handoff.require_peer_id_for(&peer_meerkat_id, &peer_key)?;
            let local_trust_created = match self
                .apply_trusted_peer_add_report(
                    local_comms.as_ref(),
                    peer_spec.clone(),
                    handoff.wiring_authority_for(
                        &peer_meerkat_id,
                        &peer_key,
                        &self.dsl_authority,
                    )?,
                )
                .await
            {
                Ok(created) => created,
                Err(error) => {
                    self.rollback_peer_only_wire(
                        &edge,
                        dsl_added,
                        &[],
                        &local,
                        &peer_meerkat_id,
                        local_spec,
                        peer_spec,
                    )
                    .await;
                    return Err(MobError::from(error));
                }
            };
            if let Err(error) = self
                .wire_peer_only_recipient(
                    peer_spec,
                    Some(peer_binding),
                    local_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                if local_trust_created {
                    let rollback_handoff = self.authorize_member_trust_unwiring(
                        &edge,
                        "wire_members_peer_only_rollback_trust_authority",
                    )?;
                    let _ = self
                        .apply_trusted_peer_remove(
                            local_comms.as_ref(),
                            peer_key.clone(),
                            rollback_handoff.unwiring_authority_for(&peer_meerkat_id, &peer_key)?,
                        )
                        .await;
                }
                self.rollback_peer_only_wire(
                    &edge,
                    dsl_added,
                    &[],
                    &local,
                    &peer_meerkat_id,
                    local_spec,
                    peer_spec,
                )
                .await;
                return Err(error);
            }
            let event = NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MembersWired {
                    a: AgentIdentity::from(edge.a.0.as_str()),
                    b: AgentIdentity::from(edge.b.0.as_str()),
                },
            };
            let stored = match self.events.append(event).await {
                Ok(stored) => stored,
                Err(error) => {
                    if local_trust_created {
                        let rollback_handoff = self.authorize_member_trust_unwiring(
                            &edge,
                            "wire_members_peer_only_event_rollback_trust_authority",
                        )?;
                        let _ = self
                            .apply_trusted_peer_remove(
                                local_comms.as_ref(),
                                peer_key.clone(),
                                rollback_handoff
                                    .unwiring_authority_for(&peer_meerkat_id, &peer_key)?,
                            )
                            .await;
                    }
                    self.rollback_peer_only_wire(
                        &edge,
                        dsl_added,
                        &["peer"],
                        &local,
                        &peer_meerkat_id,
                        local_spec,
                        peer_spec,
                    )
                    .await;
                    return Err(MobError::from(error));
                }
            };
            self.roster.write().await.apply_event(&stored);
            return Ok(());
        }
        if let (
            WiringEndpoint::PeerOnly {
                spec: local_spec,
                binding: local_binding,
            },
            WiringEndpoint::Local {
                comms: peer_comms,
                spec: peer_spec,
                ..
            },
        ) = (&local_endpoint, &peer_endpoint)
        {
            let authority = self.apply_wire_members_idempotent(&edge)?;
            let dsl_added = authority.dsl_added();
            let handoff = authority.member_handoff()?;
            let local_key = Self::trusted_peer_removal_key(local_spec);
            handoff.require_peer_id_for(&local, &local_key)?;
            let peer_trust_created = match self
                .apply_trusted_peer_add_report(
                    peer_comms.as_ref(),
                    local_spec.clone(),
                    handoff.wiring_authority_for(&local, &local_key, &self.dsl_authority)?,
                )
                .await
            {
                Ok(created) => created,
                Err(error) => {
                    self.rollback_peer_only_wire(
                        &edge,
                        dsl_added,
                        &[],
                        &local,
                        &peer_meerkat_id,
                        local_spec,
                        peer_spec,
                    )
                    .await;
                    return Err(MobError::from(error));
                }
            };
            if let Err(error) = self
                .wire_peer_only_recipient(
                    local_spec,
                    Some(local_binding),
                    peer_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                if peer_trust_created {
                    let rollback_handoff = self.authorize_member_trust_unwiring(
                        &edge,
                        "wire_members_peer_only_rollback_trust_authority",
                    )?;
                    let _ = self
                        .apply_trusted_peer_remove(
                            peer_comms.as_ref(),
                            local_key.clone(),
                            rollback_handoff.unwiring_authority_for(&local, &local_key)?,
                        )
                        .await;
                }
                self.rollback_peer_only_wire(
                    &edge,
                    dsl_added,
                    &[],
                    &local,
                    &peer_meerkat_id,
                    local_spec,
                    peer_spec,
                )
                .await;
                return Err(error);
            }
            let event = NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MembersWired {
                    a: AgentIdentity::from(edge.a.0.as_str()),
                    b: AgentIdentity::from(edge.b.0.as_str()),
                },
            };
            let stored = match self.events.append(event).await {
                Ok(stored) => stored,
                Err(error) => {
                    if peer_trust_created {
                        let rollback_handoff = self.authorize_member_trust_unwiring(
                            &edge,
                            "wire_members_peer_only_event_rollback_trust_authority",
                        )?;
                        let _ = self
                            .apply_trusted_peer_remove(
                                peer_comms.as_ref(),
                                local_key.clone(),
                                rollback_handoff.unwiring_authority_for(&local, &local_key)?,
                            )
                            .await;
                    }
                    self.rollback_peer_only_wire(
                        &edge,
                        dsl_added,
                        &["local"],
                        &local,
                        &peer_meerkat_id,
                        local_spec,
                        peer_spec,
                    )
                    .await;
                    return Err(MobError::from(error));
                }
            };
            self.roster.write().await.apply_event(&stored);
            return Ok(());
        }
        let (local_comms, local_spec) = match local_endpoint {
            WiringEndpoint::Local { comms, spec, .. } => (comms, spec),
            WiringEndpoint::PeerOnly { .. } => {
                return Err(MobError::WiringError(format!(
                    "wire requires local session comms runtime for '{local}'"
                )));
            }
        };
        let (peer_comms, peer_spec) = match peer_endpoint {
            WiringEndpoint::Local { comms, spec, .. } => (comms, spec),
            WiringEndpoint::PeerOnly { .. } => {
                return Err(MobError::WiringError(format!(
                    "wire requires local session comms runtime for '{peer_identity}'"
                )));
            }
        };

        // Submit the DSL input. A new edge emits `WiringGraphChanged`; an
        // existing edge emits generated local repair authority. Only a graph
        // change means rollback must undo a machine graph mutation.
        let authority = self.apply_wire_members_idempotent(&edge)?;
        let dsl_added = authority.dsl_added();
        let handoff = authority.member_handoff()?;

        let local_peer_id = Self::trusted_peer_removal_key(&local_spec);
        let peer_peer_id = Self::trusted_peer_removal_key(&peer_spec);

        // A-side trust install.
        handoff.require_peer_id_for(&peer_meerkat_id, &peer_peer_id)?;
        let local_trust_created = match self
            .apply_trusted_peer_add_report(
                local_comms.as_ref(),
                peer_spec.clone(),
                handoff.wiring_authority_for(
                    &peer_meerkat_id,
                    &peer_peer_id,
                    &self.dsl_authority,
                )?,
            )
            .await
        {
            Ok(created) => created,
            Err(err) => {
                self.rollback_wire_side_effects(
                    &edge,
                    dsl_added,
                    false,
                    false,
                    &local_comms,
                    &peer_comms,
                    &local_peer_id,
                    &peer_peer_id,
                    handoff,
                )
                .await;
                return Err(MobError::from(err));
            }
        };

        // B-side trust install.
        handoff.require_peer_id_for(&local, &local_peer_id)?;
        let peer_trust_created = match self
            .apply_trusted_peer_add_report(
                peer_comms.as_ref(),
                local_spec.clone(),
                handoff.wiring_authority_for(&local, &local_peer_id, &self.dsl_authority)?,
            )
            .await
        {
            Ok(created) => created,
            Err(err) => {
                self.rollback_wire_side_effects(
                    &edge,
                    dsl_added,
                    local_trust_created,
                    false,
                    &local_comms,
                    &peer_comms,
                    &local_peer_id,
                    &peer_peer_id,
                    handoff,
                )
                .await;
                return Err(MobError::from(err));
            }
        };

        // Notify A that B is now wired.
        if let Err(err) = self
            .notify_peer_added(&peer_comms, &local_spec, &peer_meerkat_id, &peer_entry)
            .await
        {
            self.rollback_wire_side_effects(
                &edge,
                dsl_added,
                local_trust_created,
                peer_trust_created,
                &local_comms,
                &peer_comms,
                &local_peer_id,
                &peer_peer_id,
                handoff,
            )
            .await;
            return Err(err);
        }

        // Notify B that A is now wired.
        if let Err(err) = self
            .notify_peer_added(&local_comms, &peer_spec, &local, &local_entry)
            .await
        {
            self.rollback_wire_side_effects(
                &edge,
                dsl_added,
                local_trust_created,
                peer_trust_created,
                &local_comms,
                &peer_comms,
                &local_peer_id,
                &peer_peer_id,
                handoff,
            )
            .await;
            return Err(err);
        }

        // Append MembersWired — rollback on failure.
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MembersWired {
                a: AgentIdentity::from(edge.a.0.as_str()),
                b: AgentIdentity::from(edge.b.0.as_str()),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(err) => {
                self.rollback_wire_side_effects(
                    &edge,
                    dsl_added,
                    local_trust_created,
                    peer_trust_created,
                    &local_comms,
                    &peer_comms,
                    &local_peer_id,
                    &peer_peer_id,
                    handoff,
                )
                .await;
                return Err(MobError::from(err));
            }
        };
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    /// Dense local-member topology materialization path.
    ///
    /// Dogma shape:
    /// - MobMachine remains the single owner of `wiring_edges`; each new edge
    ///   is staged through `WireMembersWithTrust` on an isolated authority and
    ///   published only after the durable batch event is stored.
    /// - Generated prepared-topology handoffs authorize new comms trust before
    ///   commit, but the trust writes run only after the live machine edge and
    ///   durable marker commit.
    /// - Existing edges are retried through generated trust-repair authority
    ///   rather than being treated as projection-only `already_wired` rows.
    /// - The actor owns only shell mechanics: roster snapshot validation,
    ///   comms trust installation, trust cleanup, and compact projection event.
    /// - External peers stay on the interactive single-edge path because
    ///   descriptor exchange and rollback are per-peer semantics.
    async fn handle_wire_members_batch(
        &mut self,
        requested_edges: Vec<(AgentIdentity, AgentIdentity)>,
    ) -> Result<super::handle::MobWireMembersBatchReport, MobError> {
        let requested = requested_edges.len();
        let mut normalized_edges = BTreeSet::new();
        let mut endpoint_ids = BTreeSet::new();
        for (left, right) in requested_edges {
            if left == right {
                return Err(MobError::WiringError(format!(
                    "wire_members_batch requires distinct members (got '{left}')"
                )));
            }
            let (a, b) = if left <= right {
                (left, right)
            } else {
                (right, left)
            };
            endpoint_ids.insert(a.clone());
            endpoint_ids.insert(b.clone());
            normalized_edges.insert((a, b));
        }

        if normalized_edges.is_empty() {
            return Ok(super::handle::MobWireMembersBatchReport {
                requested,
                already_wired: Vec::new(),
                wired: Vec::new(),
            });
        }

        let broken_members = self
            .dsl_authority
            .state()
            .member_restore_failures
            .keys()
            .map(|identity| AgentIdentity::from(identity.0.as_str()))
            .collect::<BTreeSet<_>>();
        for identity in &endpoint_ids {
            if broken_members.contains(identity) {
                return Err(MobError::WiringError(format!(
                    "wire_members_batch cannot wire broken member '{identity}'"
                )));
            }
        }

        let entries = {
            let roster = self.roster.read().await;
            let mut entries = BTreeMap::new();
            for identity in &endpoint_ids {
                let entry = roster
                    .get(identity)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(identity.clone()))?;
                entries.insert(identity.clone(), entry);
            }
            entries
        };

        let mut endpoints = BTreeMap::new();
        for (identity, entry) in entries {
            match self
                .resolve_wiring_endpoint(&entry, "wire_members_batch")
                .await?
            {
                WiringEndpoint::Local { comms, spec, .. } => {
                    let removal_key = Self::trusted_peer_removal_key(&spec);
                    endpoints.insert(
                        identity,
                        LocalBatchWiringEndpoint {
                            comms,
                            spec,
                            removal_key,
                        },
                    );
                }
                WiringEndpoint::PeerOnly { .. } => {
                    return Err(MobError::WiringError(format!(
                        "wire_members_batch only supports local session-backed members (got '{identity}')"
                    )));
                }
            }
        }

        let mut already_wired = Vec::new();
        let mut repair_edges = Vec::new();
        let mut to_add = Vec::new();
        for (a, b) in normalized_edges {
            let event_edge = crate::event::MemberWireEdge {
                a: a.clone(),
                b: b.clone(),
            };
            let dsl_edge = mob_dsl::WiringEdge::new(
                mob_dsl::AgentIdentity::from_domain(&a),
                mob_dsl::AgentIdentity::from_domain(&b),
            );
            if self
                .dsl_authority
                .state()
                .wiring_edges
                .iter()
                .any(|existing| existing == &dsl_edge)
            {
                already_wired.push(event_edge.clone());
                repair_edges.push((event_edge, dsl_edge));
            } else {
                to_add.push((event_edge, dsl_edge));
            }
        }

        let mut wired = Vec::with_capacity(to_add.len());
        let mut trust_applications = Vec::with_capacity((to_add.len() + repair_edges.len()) * 2);

        if !to_add.is_empty() {
            let prepared_batch_authority =
                crate::generated::protocol_mob_member_trust_wiring::MobTopologyPreparedBatchAuthority::from_live_authority(
                    &self.dsl_authority,
                );
            let mut prepared_authority = self.dsl_authority.prepare_authority();
            let mut batch_effects = Vec::new();
            let mut phase_changed = false;
            for (event_edge, dsl_edge) in &to_add {
                let input = mob_dsl::MobMachineInput::WireMembersWithTrust {
                    edge: dsl_edge.clone(),
                    a_identity: dsl_edge.a.clone(),
                    b_identity: dsl_edge.b.clone(),
                };
                let transition = match mob_dsl::MobMachineMutator::apply(
                    &mut prepared_authority,
                    input,
                ) {
                    Ok(transition) => transition,
                    Err(error) => {
                        return Err(MobError::Internal(format!(
                            "DSL authority (wire_members_batch) rejected WireMembersWithTrust for edge {dsl_edge:?}: {error}"
                        )));
                    }
                };
                let freshness_authority = prepared_batch_authority
                    .freshness_for_prepared_transitions(&self.dsl_authority, [&transition])
                    .map_err(MobError::WiringError)?;
                let handoff = match self.member_trust_wiring_handoff_from_prepared_batch_transition(
                    &transition,
                    dsl_edge,
                    freshness_authority,
                    "wire_members_batch",
                    MemberTrustOperation::Wiring,
                ) {
                    Ok(handoff) => handoff,
                    Err(error) => return Err(error),
                };
                trust_applications.extend(self.batch_wire_trust_applications_from_handoff(
                    &endpoints, event_edge, dsl_edge, &handoff,
                )?);
                if transition.from_phase != transition.to_phase {
                    phase_changed = true;
                }
                for effect in transition.effects() {
                    if let mob_dsl::MobMachineEffect::EmitWiringLifecycleNotice {
                        kind: mob_dsl::WiringLifecycleKind::Wired,
                        edge,
                    } = effect
                    {
                        wired.push(crate::event::MemberWireEdge {
                            a: AgentIdentity::from(edge.a.0.as_str()),
                            b: AgentIdentity::from(edge.b.0.as_str()),
                        });
                    }
                }
                batch_effects.extend(transition.into_effects());
            }

            let prepared = PreparedDslInput {
                authority: prepared_authority,
                effects: batch_effects,
                phase_changed,
            };

            let events = self.events.clone();
            let mob_id = self.definition.id.clone();
            let wired_for_event = wired.clone();
            let stored = self
                .commit_prepared_dsl_input_after(prepared, move || async move {
                    let event = NewMobEvent {
                        mob_id,
                        timestamp: None,
                        kind: MobEventKind::MembersWiredBatch {
                            edges: wired_for_event,
                        },
                    };
                    events.append(event).await.map_err(MobError::from)
                })
                .await?;
            self.roster.write().await.apply_event(&stored);
        }

        for (event_edge, dsl_edge) in &repair_edges {
            let handoff = self.authorize_batch_member_trust_repair(dsl_edge)?;
            trust_applications.extend(self.batch_wire_trust_applications_from_handoff(
                &endpoints, event_edge, dsl_edge, &handoff,
            )?);
        }

        let owner_token = self.dsl_authority.generated_authority_owner_token();
        self.apply_batch_wire_trust_applications(trust_applications, &owner_token)
            .await?;

        tracing::info!(
            mob_id = %self.definition.id,
            requested,
            wired = wired.len(),
            already_wired = already_wired.len(),
            participants = endpoints.len(),
            "wire_members_batch materialized local topology"
        );

        Ok(super::handle::MobWireMembersBatchReport {
            requested,
            already_wired,
            wired,
        })
    }

    async fn prepare_send_peer_message(
        &mut self,
        from: MeerkatId,
        to: MeerkatId,
        content: ContentInput,
        handling_mode: meerkat_core::types::HandlingMode,
    ) -> Result<PeerMessageDeliveryPlan, MobError> {
        self.require_state(&[MobState::Running])?;
        if from == to {
            return Err(MobError::WiringError(format!(
                "peer message requires distinct members (got '{from}')"
            )));
        }
        self.ensure_member_not_broken(&from).await?;
        self.ensure_member_not_broken(&to).await?;

        let (sender_entry, recipient_entry) = {
            let roster = self.roster.read().await;
            (
                roster
                    .get(&from)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(from.clone()))?,
                roster
                    .get(&to)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(to.clone()))?,
            )
        };

        let edge = mob_dsl::WiringEdge::new(
            mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(from.as_str())),
            mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(to.as_str())),
        );
        let wired_by_machine = self
            .dsl_authority
            .state()
            .wiring_edges
            .iter()
            .any(|existing| existing == &edge);
        if !wired_by_machine {
            return Err(MobError::WiringError(format!(
                "peer message requires wired members '{from}' and '{to}'"
            )));
        }

        let sender_comms = self
            .provisioner_comms(&sender_entry.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "peer message requires sender comms runtime for '{from}'"
                ))
            })?;
        let recipient_endpoint = self
            .resolve_wiring_endpoint(&recipient_entry, "peer_message")
            .await?;
        let recipient_spec = match recipient_endpoint {
            WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
        };
        let route =
            PeerRoute::with_display_name(recipient_spec.peer_id, recipient_spec.name.clone());
        let (body, blocks) = match content {
            ContentInput::Text(body) => (body, None),
            ContentInput::Blocks(blocks) => {
                (meerkat_core::types::text_content(&blocks), Some(blocks))
            }
        };

        Ok(PeerMessageDeliveryPlan {
            from,
            to,
            sender_comms,
            command: CommsCommand::PeerMessage {
                to: route,
                body,
                blocks,
                handling_mode,
            },
        })
    }

    fn apply_wire_members_idempotent(
        &mut self,
        edge: &mob_dsl::WiringEdge,
    ) -> Result<WireTrustAuthority, MobError> {
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::WireMembers { edge: edge.clone() },
            "wire_members",
        )?;
        let graph_added =
            Self::wire_members_disposition_from_effects(&effects, edge, "wire_members")?;
        let handoff = self.authorize_member_trust_wiring(
            edge,
            "wire_members_trust_authority",
            if graph_added {
                MemberTrustOperation::Wiring
            } else {
                MemberTrustOperation::Repair
            },
        )?;
        if graph_added {
            Ok(WireTrustAuthority::GraphAdded(handoff))
        } else {
            Ok(WireTrustAuthority::RepairRequested(handoff))
        }
    }

    fn authorize_batch_member_trust_repair(
        &mut self,
        edge: &mob_dsl::WiringEdge,
    ) -> Result<MemberTrustHandoff, MobError> {
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::WireMembersWithTrust {
                edge: edge.clone(),
                a_identity: edge.a.clone(),
                b_identity: edge.b.clone(),
            },
            "wire_members_batch_repair_request",
        )?;
        let graph_added = Self::wire_members_disposition_from_effects(
            transition.effects(),
            edge,
            "wire_members_batch_repair_request",
        )?;
        if graph_added {
            return Err(MobError::WiringError(
                "wire_members_batch repair unexpectedly produced a generated graph change"
                    .to_string(),
            ));
        }
        self.authorize_member_trust_wiring(
            edge,
            "wire_members_batch_repair_trust_authority",
            MemberTrustOperation::Repair,
        )
    }

    fn batch_wire_trust_applications_from_handoff(
        &self,
        endpoints: &BTreeMap<AgentIdentity, LocalBatchWiringEndpoint>,
        event_edge: &crate::event::MemberWireEdge,
        dsl_edge: &mob_dsl::WiringEdge,
        handoff: &MemberTrustHandoff,
    ) -> Result<Vec<BatchWireTrustApplication>, MobError> {
        let left = endpoints.get(&event_edge.a).ok_or_else(|| {
            MobError::WiringError(format!(
                "wire_members_batch missing endpoint '{}'",
                event_edge.a
            ))
        })?;
        let right = endpoints.get(&event_edge.b).ok_or_else(|| {
            MobError::WiringError(format!(
                "wire_members_batch missing endpoint '{}'",
                event_edge.b
            ))
        })?;

        let right_peer_id = right.removal_key.clone();
        handoff.require_peer_id_for(&event_edge.b, &right_peer_id)?;
        let left_trust_authority =
            handoff.add_authority_for(&event_edge.b, &right_peer_id, &self.dsl_authority)?;

        let left_peer_id = left.removal_key.clone();
        handoff.require_peer_id_for(&event_edge.a, &left_peer_id)?;
        let right_trust_authority =
            handoff.add_authority_for(&event_edge.a, &left_peer_id, &self.dsl_authority)?;

        Ok(vec![
            BatchWireTrustApplication {
                edge: dsl_edge.clone(),
                identity: event_edge.b.clone(),
                peer_id: right_peer_id,
                comms: left.comms.clone(),
                peer: right.spec.clone(),
                authority: left_trust_authority,
            },
            BatchWireTrustApplication {
                edge: dsl_edge.clone(),
                identity: event_edge.a.clone(),
                peer_id: left_peer_id,
                comms: right.comms.clone(),
                peer: left.spec.clone(),
                authority: right_trust_authority,
            },
        ])
    }

    async fn apply_batch_wire_trust_applications(
        &mut self,
        applications: Vec<BatchWireTrustApplication>,
        owner_token: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), MobError> {
        let mut installed = Vec::new();
        for application in applications {
            let rollback = BatchWireTrustRollback {
                edge: application.edge.clone(),
                identity: application.identity.clone(),
                peer_id: application.peer_id.clone(),
                comms: application.comms.clone(),
            };
            match Self::apply_trusted_peer_add_with_owner_token_report(
                application.comms.as_ref(),
                application.peer,
                application.authority,
                owner_token,
            )
            .await
            {
                Ok(created) => {
                    if created {
                        installed.push(rollback);
                    }
                }
                Err(error) => {
                    self.rollback_batch_wire_trust_applications(installed, owner_token)
                        .await;
                    return Err(MobError::from(error));
                }
            }
        }
        Ok(())
    }

    async fn rollback_batch_wire_trust_applications(
        &mut self,
        installed: Vec<BatchWireTrustRollback>,
        owner_token: &Arc<dyn std::any::Any + Send + Sync>,
    ) {
        for rollback in installed.into_iter().rev() {
            let handoff = match self.authorize_member_trust_unwiring(
                &rollback.edge,
                "wire_members_batch_trust_failure_rollback",
            ) {
                Ok(handoff) => handoff,
                Err(error) => {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        %error,
                        "wire_members_batch could not authorize trust rollback after trust failure"
                    );
                    continue;
                }
            };
            let authority =
                match handoff.unwiring_authority_for(&rollback.identity, &rollback.peer_id) {
                    Ok(authority) => authority,
                    Err(error) => {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            %error,
                            "wire_members_batch could not derive generated trust rollback authority"
                        );
                        continue;
                    }
                };
            if let Err(error) = Self::apply_trusted_peer_remove_with_owner_token(
                rollback.comms.as_ref(),
                rollback.peer_id,
                authority,
                owner_token,
            )
            .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %error,
                    "wire_members_batch failed to rollback previously installed trust after trust failure"
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn rollback_peer_only_wire(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        dsl_added: bool,
        installed_sides: &[&str],
        _local_identity: &AgentIdentity,
        _peer_identity: &AgentIdentity,
        local_spec: &TrustedPeerDescriptor,
        peer_spec: &TrustedPeerDescriptor,
    ) {
        if !dsl_added {
            return;
        }
        if let Err(error) = self.apply_dsl_input(
            mob_dsl::MobMachineInput::UnwireMembers { edge: edge.clone() },
            "peer_only_wire_rollback",
        ) {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "peer-only wire rollback: failed to revert DSL wire"
            );
            return;
        }
        if installed_sides.contains(&"local")
            && let Err(error) = self
                .unwire_peer_only_recipient(
                    local_spec,
                    None,
                    peer_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "peer-only wire rollback: failed to unwire local recipient"
            );
        }
        if installed_sides.contains(&"peer")
            && let Err(error) = self
                .unwire_peer_only_recipient(
                    peer_spec,
                    None,
                    local_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "peer-only wire rollback: failed to unwire peer recipient"
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn rollback_peer_only_unwire(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        sides_to_rewire: &[&str],
        _local_identity: &AgentIdentity,
        _peer_identity: &AgentIdentity,
        local_spec: &TrustedPeerDescriptor,
        local_binding: &crate::RuntimeBinding,
        peer_spec: &TrustedPeerDescriptor,
        peer_binding: &crate::RuntimeBinding,
    ) {
        if let Err(error) = self.apply_wire_members_idempotent(edge) {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "peer-only unwire rollback: failed to restore MobMachine wire"
            );
            return;
        }
        if sides_to_rewire.contains(&"local") {
            if let Err(error) = self
                .wire_peer_only_recipient(
                    local_spec,
                    Some(local_binding),
                    peer_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %error,
                    "peer-only unwire rollback: failed to rewire local recipient"
                );
            }
        }
        if sides_to_rewire.contains(&"peer") {
            if let Err(error) = self
                .wire_peer_only_recipient(
                    peer_spec,
                    Some(peer_binding),
                    local_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %error,
                    "peer-only unwire rollback: failed to rewire peer recipient"
                );
            }
        }
    }

    /// Unwind side effects from a failed local-local wire. Best-effort:
    /// logs on compensation failure since we've already decided to surface
    /// the original error to the caller.
    #[allow(clippy::too_many_arguments)]
    async fn rollback_wire_side_effects(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        dsl_added: bool,
        installed_local_trust: bool,
        installed_peer_trust: bool,
        local_comms: &Arc<dyn CoreCommsRuntime>,
        peer_comms: &Arc<dyn CoreCommsRuntime>,
        local_peer_id: &str,
        peer_peer_id: &str,
        _handoff: &MemberTrustHandoff,
    ) {
        let local_identity = AgentIdentity::from(edge.a.0.as_str());
        let peer_identity = AgentIdentity::from(edge.b.0.as_str());
        let rollback_handoff = if installed_local_trust || installed_peer_trust {
            match self
                .authorize_member_trust_unwiring(edge, "wire_members_rollback_trust_authority")
            {
                Ok(handoff) => Some(handoff),
                Err(err) => {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        %err,
                        "wire rollback: failed to obtain generated trust removal authority"
                    );
                    None
                }
            }
        } else {
            None
        };
        if installed_local_trust {
            if let Err(err) = self.apply_trusted_peer_remove(
                local_comms.as_ref(),
                peer_peer_id.to_string(),
                match rollback_handoff
                    .as_ref()
                    .ok_or_else(|| {
                        MobError::WiringError(
                            "wire rollback has no generated local trust removal authority"
                                .to_string(),
                        )
                    })
                    .and_then(|handoff| {
                        handoff.unwiring_authority_for(&peer_identity, peer_peer_id)
                    }) {
                    Ok(authority) => authority,
                    Err(err) => {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            %err,
                            "wire rollback: failed to build generated local trust removal authority"
                        );
                        return;
                    }
                },
            )
            .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "wire rollback: failed to remove local trust"
                );
            }
        }
        if installed_peer_trust {
            if let Err(err) = self.apply_trusted_peer_remove(
                peer_comms.as_ref(),
                local_peer_id.to_string(),
                match rollback_handoff
                    .as_ref()
                    .ok_or_else(|| {
                        MobError::WiringError(
                            "wire rollback has no generated peer trust removal authority"
                                .to_string(),
                        )
                    })
                    .and_then(|handoff| {
                        handoff.unwiring_authority_for(&local_identity, local_peer_id)
                    }) {
                    Ok(authority) => authority,
                    Err(err) => {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            %err,
                            "wire rollback: failed to build generated peer trust removal authority"
                        );
                        return;
                    }
                },
            )
            .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "wire rollback: failed to remove peer trust"
                );
            }
        }
        if dsl_added {
            if let Err(err) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::UnwireMembers { edge: edge.clone() },
                "wire_members_rollback",
            ) {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "wire rollback: failed to revert DSL wire"
                );
            }
        }
    }

    /// D-wire-handler (#26): forward an unwire command to the MobMachine DSL.
    ///
    /// Mirror of [`handle_wire`]: submits
    /// `MobMachineInput::UnwireMembers { edge }` and records
    /// `MobEventKind::MembersUnwired { a, b }` on acceptance. Already-absent
    /// idempotency is a generated no-op transition.
    async fn handle_unwire(
        &mut self,
        local: MeerkatId,
        target: super::handle::PeerTarget,
    ) -> Result<(), MobError> {
        let peer_identity = match target {
            super::handle::PeerTarget::Local(id) => {
                // Callers commonly unwire an external peer by name projected
                // as a Local target. The machine-owned external edge decides
                // that route; the roster projection is display-only.
                let target_identity =
                    mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(id.as_str()));
                let target_is_member = self
                    .dsl_authority
                    .state()
                    .identity_to_runtime
                    .contains_key(&target_identity);
                let external_peer_name =
                    meerkat_core::comms::PeerName::new(id.as_str().to_string())
                        .ok()
                        .filter(|peer_name| {
                            let local_identity = AgentIdentity::from(local.as_str());
                            self.external_peer_edge_for_name(&local_identity, peer_name)
                                .is_some()
                        });
                if !target_is_member && let Some(peer_name) = external_peer_name {
                    return self.handle_unwire_external(local, peer_name, None).await;
                }
                id
            }
            super::handle::PeerTarget::ExternalName(peer_name) => {
                return self.handle_unwire_external(local, peer_name, None).await;
            }
            super::handle::PeerTarget::ExternalBinding(binding) => {
                let descriptor = Self::trusted_peer_descriptor_from_external_binding(binding)?;
                let peer_name = descriptor.name.clone();
                return self
                    .handle_unwire_external(local, peer_name, Some(descriptor))
                    .await;
            }
            super::handle::PeerTarget::External(descriptor) => {
                let peer_name = descriptor.name.clone();
                return self
                    .handle_unwire_external(local, peer_name, Some(descriptor))
                    .await;
            }
        };

        let local_identity = AgentIdentity::from(local.as_str());
        if local_identity == peer_identity {
            return Err(MobError::WiringError(format!(
                "unwire requires distinct peers (got '{local}')"
            )));
        }

        let peer_meerkat_id = MeerkatId::from(peer_identity.as_str());
        let dsl_a = mob_dsl::AgentIdentity::from_domain(&local_identity);
        let dsl_b = mob_dsl::AgentIdentity::from_domain(&peer_identity);
        let edge = mob_dsl::WiringEdge::new(dsl_a, dsl_b);

        let (local_entry, peer_entry) = {
            let roster = self.roster.read().await;
            (
                roster
                    .get(&local)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(local.clone()))?,
                roster
                    .get(&peer_meerkat_id)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(peer_meerkat_id.clone()))?,
            )
        };

        let dsl_has_edge = self
            .dsl_authority
            .state()
            .wiring_edges
            .iter()
            .any(|existing| existing == &edge);

        // Resolve endpoints. Failing here leaves the DSL + roster
        // untouched — matches the zero-side-effect expectations.
        let local_endpoint = self.resolve_wiring_endpoint(&local_entry, "unwire").await?;
        let peer_endpoint = self.resolve_wiring_endpoint(&peer_entry, "unwire").await?;
        if let (
            WiringEndpoint::PeerOnly {
                spec: local_spec,
                binding: local_binding,
            },
            WiringEndpoint::PeerOnly {
                spec: peer_spec,
                binding: peer_binding,
            },
        ) = (&local_endpoint, &peer_endpoint)
        {
            if !dsl_has_edge {
                return Err(MobError::WiringError(format!(
                    "peer-only unwire for '{local}' <-> '{peer_identity}' requires MobMachine wiring authority"
                )));
            }

            self.cancel_peer_deliveries_for_edge(&local, &peer_meerkat_id, "members are unwiring")
                .await;
            let _unwire_handoff = self.apply_unwire_members_idempotent(&edge)?;
            if let Err(error) = self
                .unwire_peer_only_recipient(
                    local_spec,
                    Some(local_binding),
                    peer_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                self.rollback_peer_only_unwire(
                    &edge,
                    &[],
                    &local,
                    &peer_meerkat_id,
                    local_spec,
                    local_binding,
                    peer_spec,
                    peer_binding,
                )
                .await;
                return Err(error);
            }
            if let Err(error) = self
                .unwire_peer_only_recipient(
                    peer_spec,
                    Some(peer_binding),
                    local_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                self.rollback_peer_only_unwire(
                    &edge,
                    &["local"],
                    &local,
                    &peer_meerkat_id,
                    local_spec,
                    local_binding,
                    peer_spec,
                    peer_binding,
                )
                .await;
                return Err(error);
            }

            let event = NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MembersUnwired {
                    a: AgentIdentity::from(edge.a.0.as_str()),
                    b: AgentIdentity::from(edge.b.0.as_str()),
                },
            };
            let stored = match self.events.append(event).await {
                Ok(stored) => stored,
                Err(error) => {
                    self.rollback_peer_only_unwire(
                        &edge,
                        &["local", "peer"],
                        &local,
                        &peer_meerkat_id,
                        local_spec,
                        local_binding,
                        peer_spec,
                        peer_binding,
                    )
                    .await;
                    return Err(MobError::from(error));
                }
            };
            self.roster.write().await.apply_event(&stored);
            return Ok(());
        }
        let (local_comms, local_spec) = match local_endpoint {
            WiringEndpoint::Local { comms, spec, .. } => (comms, spec),
            WiringEndpoint::PeerOnly { .. } => {
                return Err(MobError::WiringError(format!(
                    "unwire requires local session comms runtime for '{local}'"
                )));
            }
        };
        let (peer_comms, peer_spec) = match peer_endpoint {
            WiringEndpoint::Local { comms, spec, .. } => (comms, spec),
            WiringEndpoint::PeerOnly { .. } => {
                return Err(MobError::WiringError(format!(
                    "unwire requires local session comms runtime for '{peer_identity}'"
                )));
            }
        };

        let local_peer_id = Self::trusted_peer_removal_key(&local_spec);
        let peer_peer_id = Self::trusted_peer_removal_key(&peer_spec);

        // Idempotent absence stays a no-op only when live comms agrees.
        // Stale trust cleanup needs MobMachine authority instead of using the
        // roster projection as a behavior owner.
        if !dsl_has_edge {
            let local_has_stale_trust = local_comms
                .peers()
                .await
                .iter()
                .any(|peer| peer.peer_id.to_string() == peer_peer_id);
            let peer_has_stale_trust = peer_comms
                .peers()
                .await
                .iter()
                .any(|peer| peer.peer_id.to_string() == local_peer_id);
            if local_has_stale_trust || peer_has_stale_trust {
                return Err(MobError::WiringError(format!(
                    "unwire for '{local}' <-> '{peer_identity}' requires MobMachine wiring authority"
                )));
            }
            return Ok(());
        }

        self.cancel_peer_deliveries_for_edge(&local, &peer_meerkat_id, "members are unwiring")
            .await;

        // Submit DSL input first. Already-absent idempotency is a generated
        // no-op transition; only `WiringGraphChanged` means rollback must
        // re-submit the wire.
        let unwire_handoff = self.apply_unwire_members_idempotent(&edge)?;
        let dsl_removed = true;

        let mut removed_local_trust = false;
        let mut removed_peer_trust = false;
        let mut sent_unwired_from_local = false;
        let mut sent_unwired_from_peer = false;

        // Notify peer_unwired on both sides BEFORE trust removal so the
        // notifications can still be delivered (the send path resolves
        // the recipient by name in the comms' trusted-peers table). If a
        // notification fails, compensate the prior side's notification
        // by re-sending peer_added so the observable intent stream stays
        // balanced.
        if let Err(err) = self
            .notify_peer_event(
                "mob.peer_unwired",
                &peer_spec,
                &peer_meerkat_id,
                &peer_entry,
                &local_comms,
            )
            .await
        {
            self.rollback_unwire_side_effects(
                &edge,
                dsl_removed,
                removed_local_trust,
                removed_peer_trust,
                sent_unwired_from_local,
                sent_unwired_from_peer,
                &local_comms,
                &peer_comms,
                &local_spec,
                &peer_spec,
                &local,
                &peer_meerkat_id,
                &local_entry,
                &peer_entry,
                &unwire_handoff,
            )
            .await;
            return Err(err);
        }
        sent_unwired_from_local = true;

        if let Err(err) = self
            .notify_peer_event(
                "mob.peer_unwired",
                &local_spec,
                &local,
                &local_entry,
                &peer_comms,
            )
            .await
        {
            self.rollback_unwire_side_effects(
                &edge,
                dsl_removed,
                removed_local_trust,
                removed_peer_trust,
                sent_unwired_from_local,
                sent_unwired_from_peer,
                &local_comms,
                &peer_comms,
                &local_spec,
                &peer_spec,
                &local,
                &peer_meerkat_id,
                &local_entry,
                &peer_entry,
                &unwire_handoff,
            )
            .await;
            return Err(err);
        }
        sent_unwired_from_peer = true;

        // A-side trust removal (after notifications succeeded).
        unwire_handoff.require_peer_id_for(&peer_meerkat_id, &peer_peer_id)?;
        if let Err(err) = self
            .apply_trusted_peer_remove(
                local_comms.as_ref(),
                peer_peer_id.clone(),
                unwire_handoff.unwiring_authority_for(&peer_meerkat_id, &peer_peer_id)?,
            )
            .await
        {
            self.rollback_unwire_side_effects(
                &edge,
                dsl_removed,
                removed_local_trust,
                removed_peer_trust,
                sent_unwired_from_local,
                sent_unwired_from_peer,
                &local_comms,
                &peer_comms,
                &local_spec,
                &peer_spec,
                &local,
                &peer_meerkat_id,
                &local_entry,
                &peer_entry,
                &unwire_handoff,
            )
            .await;
            return Err(MobError::from(err));
        }
        removed_local_trust = true;

        // B-side trust removal.
        unwire_handoff.require_peer_id_for(&local, &local_peer_id)?;
        if let Err(err) = self
            .apply_trusted_peer_remove(
                peer_comms.as_ref(),
                local_peer_id.clone(),
                unwire_handoff.unwiring_authority_for(&local, &local_peer_id)?,
            )
            .await
        {
            self.rollback_unwire_side_effects(
                &edge,
                dsl_removed,
                removed_local_trust,
                removed_peer_trust,
                sent_unwired_from_local,
                sent_unwired_from_peer,
                &local_comms,
                &peer_comms,
                &local_spec,
                &peer_spec,
                &local,
                &peer_meerkat_id,
                &local_entry,
                &peer_entry,
                &unwire_handoff,
            )
            .await;
            return Err(MobError::from(err));
        }
        removed_peer_trust = true;

        // Append MembersUnwired — rollback on failure.
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MembersUnwired {
                a: AgentIdentity::from(edge.a.0.as_str()),
                b: AgentIdentity::from(edge.b.0.as_str()),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(err) => {
                self.rollback_unwire_side_effects(
                    &edge,
                    dsl_removed,
                    removed_local_trust,
                    removed_peer_trust,
                    sent_unwired_from_local,
                    sent_unwired_from_peer,
                    &local_comms,
                    &peer_comms,
                    &local_spec,
                    &peer_spec,
                    &local,
                    &peer_meerkat_id,
                    &local_entry,
                    &peer_entry,
                    &unwire_handoff,
                )
                .await;
                return Err(MobError::from(err));
            }
        };
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    fn apply_unwire_members_idempotent(
        &mut self,
        edge: &mob_dsl::WiringEdge,
    ) -> Result<MemberTrustHandoff, MobError> {
        let handoff =
            self.authorize_member_trust_unwiring(edge, "unwire_members_trust_authority")?;
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::UnwireMembers { edge: edge.clone() },
            "unwire_members",
        )?;
        if Self::effects_include_wiring_graph_change(&effects) {
            Ok(handoff)
        } else {
            Err(MobError::WiringError(format!(
                "unwire_members produced no generated wiring graph authority for edge {edge:?}"
            )))
        }
    }

    fn external_peer_edge(
        local_identity: &AgentIdentity,
        spec: &TrustedPeerDescriptor,
    ) -> mob_dsl::ExternalPeerEdge {
        mob_dsl::ExternalPeerEdge::new(
            mob_dsl::AgentIdentity::from_domain(local_identity),
            mob_dsl::ExternalPeerEndpoint::from(spec),
        )
    }

    fn external_peer_key(
        local_identity: &AgentIdentity,
        peer_name: &meerkat_core::comms::PeerName,
    ) -> mob_dsl::ExternalPeerKey {
        mob_dsl::ExternalPeerKey::new(
            mob_dsl::AgentIdentity::from_domain(local_identity),
            mob_dsl::PeerName::from(peer_name.as_str()),
        )
    }

    fn external_peer_key_for_edge(edge: &mob_dsl::ExternalPeerEdge) -> mob_dsl::ExternalPeerKey {
        mob_dsl::ExternalPeerKey::new(edge.local.clone(), edge.endpoint.name.clone())
    }

    fn external_peer_edge_for_name(
        &self,
        local_identity: &AgentIdentity,
        peer_name: &meerkat_core::comms::PeerName,
    ) -> Option<mob_dsl::ExternalPeerEdge> {
        let key = Self::external_peer_key(local_identity, peer_name);
        self.dsl_authority
            .state()
            .external_peer_edges_by_key
            .get(&key)
            .cloned()
    }

    fn apply_wire_external_peer_idempotent(
        &mut self,
        key: &mob_dsl::ExternalPeerKey,
        edge: &mob_dsl::ExternalPeerEdge,
    ) -> Result<WireTrustAuthority, MobError> {
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::WireExternalPeer {
                key: key.clone(),
                edge: edge.clone(),
            },
            "wire_external_peer",
        )?;
        self.wire_external_authority_from_transition(&transition, edge, "wire_external_peer")
    }

    async fn apply_external_peer_reciprocal_trust(
        &mut self,
        key: mob_dsl::ExternalPeerKey,
        target_comms: Arc<dyn CoreCommsRuntime>,
        peer: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let local_identity = AgentIdentity::from(key.local.0.as_str());
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::AuthorizeExternalPeerReciprocalTrust {
                key: key.clone(),
                agent_identity: mob_dsl::AgentIdentity::from_domain(&local_identity),
            },
            "apply_external_peer_reciprocal_trust",
        )?;
        let Some(obligation) =
            crate::generated::protocol_mob_external_peer_reciprocal_trust::extract_obligations_with_freshness(
                &transition,
                crate::generated::protocol_mob_external_peer_reciprocal_trust::MobTopologyFreshnessAuthority::from_live_topology_epoch(self.dsl_topology_epoch.clone(), Arc::clone(&self.dsl_authority_owner_token)),
            )
            .into_iter()
            .find(|obligation| obligation.key() == &key)
        else {
            return Err(MobError::WiringError(
                "MobMachine produced no external reciprocal trust obligation".to_string(),
            ));
        };
        let target_peer_id = target_comms.peer_id().ok_or_else(|| {
            MobError::WiringError(
                "external reciprocal trust target runtime did not expose peer_id".to_string(),
            )
        })?;
        if target_peer_id.to_string() != obligation.edge().endpoint.peer_id.0 {
            return Err(MobError::WiringError(format!(
                "external reciprocal trust target peer_id {target_peer_id} does not match MobMachine external edge peer_id {}",
                obligation.edge().endpoint.peer_id.0
            )));
        }
        let peer_id = obligation.peer_id().0.clone();
        let authority = crate::generated::protocol_mob_external_peer_reciprocal_trust::reciprocal_wiring_authority_for_peer(
            &obligation,
            &peer_id,
        )
        .map_err(MobError::WiringError)?;
        self.apply_trusted_peer_add(target_comms.as_ref(), peer, authority)
            .await
            .map_err(MobError::CommsError)
    }

    fn apply_unwire_external_peer_idempotent(
        &mut self,
        key: &mob_dsl::ExternalPeerKey,
        edge: &mob_dsl::ExternalPeerEdge,
    ) -> Result<Option<CommsTrustMutationAuthority>, MobError> {
        let transition = self.apply_dsl_input_collect_transition(
            mob_dsl::MobMachineInput::UnwireExternalPeer {
                key: key.clone(),
                edge: edge.clone(),
            },
            "unwire_external_peer",
        )?;
        let effects = transition.effects();
        if !Self::effects_include_wiring_graph_change(effects) {
            return Ok(None);
        }
        let obligation =
            crate::generated::protocol_mob_external_peer_trust_unwiring::extract_obligations_with_freshness(
                &transition,
                crate::generated::protocol_mob_external_peer_trust_unwiring::MobTopologyFreshnessAuthority::from_live_topology_epoch(self.dsl_topology_epoch.clone(), Arc::clone(&self.dsl_authority_owner_token)),
            )
            .into_iter()
            .find(|obligation| obligation.edge() == edge)
            .ok_or_else(|| {
                MobError::WiringError(
                    "unwire_external_peer produced graph authority without generated unwiring trust obligation"
                        .to_string(),
                )
            })?;
        Ok(Some(
            crate::generated::protocol_mob_external_peer_trust_unwiring::unwiring_authority_for_peer(
                &obligation,
                edge.endpoint.peer_id.0.as_str(),
            )
            .map_err(MobError::WiringError)?,
        ))
    }

    /// Unwind side effects from a failed local-local unwire. Best-effort.
    ///
    /// Re-installs trust on sides where trust was removed, and emits
    /// compensating `mob.peer_added` notifications on sides that already
    /// sent `mob.peer_unwired` so the observable intent stream stays
    /// balanced. The DSL wire is re-submitted if it was removed.
    #[allow(clippy::too_many_arguments)]
    async fn rollback_unwire_side_effects(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        dsl_removed: bool,
        removed_local_trust: bool,
        removed_peer_trust: bool,
        sent_unwired_from_local: bool,
        sent_unwired_from_peer: bool,
        local_comms: &Arc<dyn CoreCommsRuntime>,
        peer_comms: &Arc<dyn CoreCommsRuntime>,
        local_spec: &TrustedPeerDescriptor,
        peer_spec: &TrustedPeerDescriptor,
        local_meerkat_id: &MeerkatId,
        peer_meerkat_id: &MeerkatId,
        local_entry: &RosterEntry,
        peer_entry: &RosterEntry,
        _handoff: &MemberTrustHandoff,
    ) {
        let rollback_handoff = if dsl_removed {
            match self.apply_wire_members_idempotent(edge) {
                Ok(authority) => match authority.member_handoff() {
                    Ok(handoff) => Some(handoff.clone()),
                    Err(err) => {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            %err,
                            "unwire rollback: generated wire authority did not include member handoff"
                        );
                        None
                    }
                },
                Err(err) => {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        %err,
                        "unwire rollback: failed to re-submit DSL wire for generated trust authority"
                    );
                    None
                }
            }
        } else {
            None
        };
        if removed_local_trust {
            let peer_key = Self::trusted_peer_removal_key(peer_spec);
            if let Err(err) = self.apply_trusted_peer_add(
                local_comms.as_ref(),
                peer_spec.clone(),
                match rollback_handoff
                    .as_ref()
                    .ok_or_else(|| {
                        MobError::WiringError(
                            "unwire rollback has no generated local trust add authority"
                                .to_string(),
                        )
                    })
                    .and_then(|handoff| {
                        handoff.add_authority_for(
                            peer_meerkat_id,
                            &peer_key,
                            &self.dsl_authority,
                        )
                    })
                {
                    Ok(authority) => authority,
                    Err(err) => {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            %err,
                            "unwire rollback: failed to build generated local trust add authority"
                        );
                        return;
                    }
                },
            )
            .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "unwire rollback: failed to re-install local trust"
                );
            }
        }
        if removed_peer_trust {
            let local_key = Self::trusted_peer_removal_key(local_spec);
            if let Err(err) = self.apply_trusted_peer_add(
                peer_comms.as_ref(),
                local_spec.clone(),
                match rollback_handoff
                    .as_ref()
                    .ok_or_else(|| {
                        MobError::WiringError(
                            "unwire rollback has no generated peer trust add authority".to_string(),
                        )
                    })
                    .and_then(|handoff| {
                        handoff.add_authority_for(
                            local_meerkat_id,
                            &local_key,
                            &self.dsl_authority,
                        )
                    })
                {
                    Ok(authority) => authority,
                    Err(err) => {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            %err,
                            "unwire rollback: failed to build generated peer trust add authority"
                        );
                        return;
                    }
                },
            )
            .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "unwire rollback: failed to re-install peer trust"
                );
            }
        }
        // Compensate peer_unwired notifications by re-sending peer_added.
        if sent_unwired_from_local {
            if let Err(err) = self
                .notify_peer_added(local_comms, peer_spec, peer_meerkat_id, peer_entry)
                .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "unwire rollback: failed to send compensating peer_added from local side"
                );
            }
        }
        if sent_unwired_from_peer {
            if let Err(err) = self
                .notify_peer_added(peer_comms, local_spec, local_meerkat_id, local_entry)
                .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "unwire rollback: failed to send compensating peer_added from peer side"
                );
            }
        }
    }

    fn trusted_peer_descriptor_from_external_binding(
        binding: super::handle::ExternalPeerBindingSpec,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let name = PeerName::new(binding.name.clone()).map_err(|error| {
            MobError::WiringError(format!(
                "external binding has invalid peer name '{}': {error}",
                binding.name
            ))
        })?;
        let resolved = binding
            .identity
            .resolve()
            .map_err(|error| MobError::WiringError(error.to_string()))?;
        let _address = PeerAddress::parse(&binding.address).map_err(|error| {
            MobError::WiringError(format!(
                "external binding has invalid peer address '{}': {error}",
                binding.address
            ))
        })?;
        TrustedPeerDescriptor::unsigned_with_pubkey(
            name.as_str().to_string(),
            resolved.peer_id.to_string(),
            resolved.pubkey,
            binding.address,
        )
        .map_err(|error| MobError::WiringError(format!("external binding is invalid: {error}")))
    }

    /// Wire a local member to an external trusted peer.
    ///
    /// `MobMachineInput::WireExternalPeer` owns the descriptor-bearing trust
    /// edge; the shell mechanically installs the admitted descriptor into the
    /// local comms runtime and projects the event only after the machine
    /// admits the edge.
    ///
    /// Idempotent: a second wire of the same (local, external_name) edge
    /// with the same descriptor is treated as a no-op success.
    async fn handle_wire_external(
        &mut self,
        local: MeerkatId,
        spec: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        TrustedPeerDescriptor::validate_pubkey_for_peer_id(spec.peer_id, &spec.pubkey).map_err(
            |error| MobError::WiringError(format!("external peer descriptor is invalid: {error}")),
        )?;
        let local_identity = AgentIdentity::from(local.as_str());
        let external_identity = AgentIdentity::from(spec.name.as_str());
        if local_identity == external_identity {
            return Err(MobError::WiringError(format!(
                "wire requires distinct members (got '{local}')"
            )));
        }
        let edge = Self::external_peer_edge(&local_identity, &spec);
        let key = Self::external_peer_key_for_edge(&edge);
        self.probe_command_admission(
            mob_dsl::MobMachineInput::WireExternalPeer {
                key: key.clone(),
                edge: edge.clone(),
            },
            MobState::Running,
            "wire_external_peer_command_admission",
        )?;

        // Look up the local member's roster entry and session binding.
        let member_ref = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(&local)
                .ok_or_else(|| MobError::MemberNotFound(local.clone()))?;
            entry.member_ref.clone()
        };

        // Resolve the local session's comms runtime for trust install.
        let comms = self.provisioner_comms(&member_ref).await.ok_or_else(|| {
            MobError::WiringError(format!(
                "wire requires comms runtime for '{local}' (external peer wire)"
            ))
        })?;

        let authority = self.apply_wire_external_peer_idempotent(&key, &edge)?;
        let removal_key = Self::trusted_peer_removal_key(&spec);
        if authority.is_repair() {
            self.apply_trusted_peer_add(
                comms.as_ref(),
                spec.clone(),
                authority.external_authority()?.clone(),
            )
            .await?;
            return Ok(());
        }

        let dsl_added = authority.dsl_added();

        // Install trust on the local's session comms runtime.
        if let Err(error) = self
            .apply_trusted_peer_add(
                comms.as_ref(),
                spec.clone(),
                authority.external_authority()?.clone(),
            )
            .await
        {
            self.rollback_external_wire_dsl(&key, &edge, dsl_added)
                .await;
            return Err(MobError::from(error));
        }

        // Append ExternalPeerWired — if the append fails, compensate by
        // rolling back the trust install so failure leaves no side effect.
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::ExternalPeerWired {
                local: local_identity,
                spec: spec.clone(),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(append_err) => {
                let rollback_handoff = if dsl_added {
                    match self.apply_unwire_external_peer_idempotent(&key, &edge) {
                        Ok(Some(handoff)) => Some(handoff),
                        Ok(None) => None,
                        Err(error) => {
                            tracing::warn!(
                                mob_id = %self.definition.id,
                                local = %local,
                                %error,
                                "failed to obtain generated external unwiring authority after event append failure"
                            );
                            None
                        }
                    }
                } else {
                    None
                };
                if let Some(rollback_handoff) = rollback_handoff {
                    if let Err(rollback_err) = self
                        .apply_trusted_peer_remove(
                            comms.as_ref(),
                            removal_key.clone(),
                            rollback_handoff,
                        )
                        .await
                    {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            local = %local,
                            error = %rollback_err,
                            "failed to rollback external trust install after event append failure"
                        );
                    }
                } else {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        local = %local,
                        "external trust install rollback skipped without generated unwiring authority"
                    );
                }
                return Err(MobError::from(append_err));
            }
        };

        // Mirror the event through the roster projection — same pattern
        // as MembersWired in `handle_wire`.
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    async fn rollback_external_wire_dsl(
        &mut self,
        key: &mob_dsl::ExternalPeerKey,
        edge: &mob_dsl::ExternalPeerEdge,
        dsl_added: bool,
    ) {
        if dsl_added
            && let Err(error) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::UnwireExternalPeer {
                    key: key.clone(),
                    edge: edge.clone(),
                },
                "external_wire_rollback",
            )
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "external wire rollback: failed to revert DSL wire"
            );
        }
    }

    /// D-external-peer (#31): unwire a local member from an external trusted peer.
    ///
    /// Mirror of [`Self::handle_wire_external`]: removes trust from the
    /// local session comms runtime, appends `ExternalPeerUnwired`, and
    /// projects through the roster.
    ///
    /// Idempotent: unwiring an already-absent external peer is a no-op
    /// success.
    async fn handle_unwire_external(
        &mut self,
        local: MeerkatId,
        peer_name: meerkat_core::comms::PeerName,
        stale_cleanup_spec: Option<TrustedPeerDescriptor>,
    ) -> Result<(), MobError> {
        let local_identity = AgentIdentity::from(local.as_str());
        // The machine-owned external edge supplies the prior descriptor.
        // The roster mirror may lag and is display-only.
        let member_ref = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(&local)
                .ok_or_else(|| MobError::MemberNotFound(local.clone()))?;
            entry.member_ref.clone()
        };

        let authority_edge = self
            .external_peer_edge_for_name(&local_identity, &peer_name)
            .or_else(|| {
                stale_cleanup_spec
                    .as_ref()
                    .map(|spec| Self::external_peer_edge(&local_identity, spec))
            });
        let prior_spec = authority_edge
            .as_ref()
            .and_then(|edge| {
                self.external_peer_edge_for_name(&local_identity, &peer_name)
                    .filter(|machine_edge| machine_edge == edge)
            })
            .map(|edge| Self::trusted_peer_descriptor_from_machine_endpoint(&edge.endpoint))
            .transpose()?;

        let Some(prior_spec) = prior_spec else {
            let unwire_handoff = match authority_edge.as_ref() {
                Some(edge) => {
                    let key = Self::external_peer_key_for_edge(edge);
                    self.apply_unwire_external_peer_idempotent(&key, edge)?
                }
                None => None,
            };
            let dsl_removed = unwire_handoff.is_some();
            if let Some(spec) = stale_cleanup_spec
                && let Some(comms) = self.provisioner_comms(&member_ref).await
            {
                let removal_key = Self::trusted_peer_removal_key(&spec);
                let has_stale_trust = comms
                    .peers()
                    .await
                    .iter()
                    .any(|peer| peer.peer_id.to_string() == removal_key);
                if !dsl_removed {
                    if has_stale_trust {
                        return Err(MobError::WiringError(format!(
                            "external unwire for '{local}' -> '{peer_name}' requires MobMachine external peer authority"
                        )));
                    }
                    return Ok(());
                }
                if let Err(error) = self.apply_trusted_peer_remove(
                    comms.as_ref(),
                    removal_key.clone(),
                    unwire_handoff
                        .as_ref()
                        .ok_or_else(|| {
                            MobError::WiringError(format!(
                                "external stale cleanup for '{local}' -> '{peer_name}' has no generated unwiring handoff"
                            ))
                        })?
                        .clone(),
                )
                .await
                {
                    if dsl_removed && let Some(edge) = authority_edge.as_ref() {
                        let key = Self::external_peer_key_for_edge(edge);
                        if let Err(rollback_err) = self.apply_dsl_input(
                            mob_dsl::MobMachineInput::WireExternalPeer {
                                key,
                                edge: edge.clone(),
                            },
                            "external_unwire_stale_cleanup_rollback",
                        ) {
                            tracing::warn!(
                                mob_id = %self.definition.id,
                                local = %local,
                                error = %rollback_err,
                                "failed to rollback external DSL unwire after stale cleanup failure"
                            );
                        }
                    }
                    return Err(MobError::from(error));
                }
            }
            return Ok(());
        };

        let edge = authority_edge
            .unwrap_or_else(|| Self::external_peer_edge(&local_identity, &prior_spec));
        let key = Self::external_peer_key_for_edge(&edge);
        let comms = self.provisioner_comms(&member_ref).await.ok_or_else(|| {
            MobError::WiringError(format!(
                "unwire requires comms runtime for '{local}' (external peer unwire)"
            ))
        })?;

        let Some(unwire_handoff) = self.apply_unwire_external_peer_idempotent(&key, &edge)? else {
            return Err(MobError::WiringError(format!(
                "external unwire for '{local}' -> '{peer_name}' was not authorized by MobMachine"
            )));
        };
        let dsl_removed = true;

        // Remove trust on the local session runtime.
        let prior_removal_key = Self::trusted_peer_removal_key(&prior_spec);
        if let Err(error) = self
            .apply_trusted_peer_remove(comms.as_ref(), prior_removal_key.clone(), unwire_handoff)
            .await
        {
            if dsl_removed
                && let Err(rollback_err) = self.apply_dsl_input(
                    mob_dsl::MobMachineInput::WireExternalPeer {
                        key: key.clone(),
                        edge: edge.clone(),
                    },
                    "external_unwire_trust_remove_rollback",
                )
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    local = %local,
                    error = %rollback_err,
                    "failed to rollback external DSL unwire after trust removal failure"
                );
            }
            return Err(MobError::from(error));
        }

        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::ExternalPeerUnwired {
                local: local_identity,
                peer_name: peer_name.clone(),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(append_err) => {
                // Restore trust on append failure so the unwire is a full
                // no-op from the caller's perspective.
                if dsl_removed {
                    match self.apply_wire_external_peer_idempotent(&key, &edge) {
                        Ok(rollback_authority) => {
                            let rollback_authority =
                                rollback_authority.external_authority()?.clone();
                            if let Err(rollback_err) = self
                                .apply_trusted_peer_add(
                                    comms.as_ref(),
                                    prior_spec.clone(),
                                    rollback_authority,
                                )
                                .await
                            {
                                tracing::warn!(
                                    mob_id = %self.definition.id,
                                    local = %local,
                                    error = %rollback_err,
                                    "failed to rollback external trust removal after event append failure"
                                );
                            }
                        }
                        Err(rollback_err) => {
                            tracing::warn!(
                                mob_id = %self.definition.id,
                                local = %local,
                                error = %rollback_err,
                                "failed to obtain generated external wiring authority after event append failure"
                            );
                        }
                    }
                }
                return Err(MobError::from(append_err));
            }
        };

        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    ///
    /// Mark-then-cleanup: event first, mark Retiring, disposal pipeline
    /// (policy-driven), then roster removal only after critical archive cleanup
    /// succeeds.
    async fn handle_retire(&mut self, agent_identity: MeerkatId) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_retire preflight")?;
        self.cancel_peer_deliveries_for_member(&agent_identity, "member is retiring")
            .await;
        self.handle_retire_inner(&agent_identity, false, false)
            .await?;
        let canceled = self
            .cancel_pending_spawns_for_member(&agent_identity, "retire command accepted")
            .await?;
        if canceled > 0 {
            tracing::info!(
                agent_identity = %agent_identity,
                canceled,
                "retire canceled pending spawn lineage after generated retirement admission"
            );
        }
        self.ensure_pending_spawn_alignment("handle_retire completion")
    }

    async fn detach_session_ingress_for_mob_destroy(
        &mut self,
        session_id: &SessionId,
        obligation: MobDestroyingSessionIngressObligation,
    ) -> Result<(), MobError> {
        use crate::generated::protocol_mob_destroying_session_ingress::{
            submit_session_ingress_detach_failed_for_mob_destroy,
            submit_session_ingress_detached_for_mob_destroy,
        };

        let detach_result = self.detach_runtime_session_ingress(session_id).await;
        match detach_result {
            Ok(()) => {
                submit_session_ingress_detached_for_mob_destroy(&mut self.dsl_authority, obligation)
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "MobMachine SessionIngressDetachedForMobDestroy transition rejected: {error}"
                        ))
                    })?;
                self.publish_machine_state_projection();
                Ok(())
            }
            Err(error) => {
                let reason = error.to_string();
                let _ = submit_session_ingress_detach_failed_for_mob_destroy(
                    &mut self.dsl_authority,
                    obligation,
                    reason.clone(),
                );
                self.publish_machine_state_projection();
                Err(MobError::Internal(format!(
                    "mob_destroying_session_ingress detach failed for {session_id}: {reason}"
                )))
            }
        }
    }

    fn acknowledge_absent_session_ingress_for_mob_destroy(
        &mut self,
        agent_identity: &MeerkatId,
        obligation: MobDestroyingSessionIngressObligation,
    ) -> Result<(), MobError> {
        crate::generated::protocol_mob_destroying_session_ingress::submit_session_ingress_detached_for_mob_destroy(
            &mut self.dsl_authority,
            obligation,
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "MobMachine SessionIngressDetachedForMobDestroy transition rejected for member '{agent_identity}' without bridge session: {error}"
            ))
        })?;
        self.publish_machine_state_projection();
        Ok(())
    }

    fn destroy_ingress_detach_session_id(
        entry: &RosterEntry,
        releasing: Option<&mob_dsl::SessionId>,
    ) -> Result<Option<SessionId>, MobError> {
        releasing
            .map(|session_id| {
                SessionId::parse(&session_id.0).map_err(|_| {
                    MobError::Internal(format!(
                        "destroy retire for member '{}' has invalid bridge session binding '{}'",
                        entry.agent_identity, session_id.0
                    ))
                })
                .map(Some)
            })
            .transpose()
            .map(Option::flatten)
    }

    async fn detach_runtime_session_ingress(&self, session_id: &SessionId) -> Result<(), MobError> {
        #[cfg(feature = "runtime-adapter")]
        if let Some(adapter) = &self.runtime_adapter {
            adapter
                .update_peer_ingress_context(session_id, false, None)
                .await;
            let owner = adapter.peer_ingress_owner(session_id).await;
            if !matches!(owner, meerkat_runtime::PeerIngressOwner::Unattached) {
                return Err(MobError::Internal(format!(
                    "peer ingress owner remained attached after detach: {owner:?}"
                )));
            }
        }
        let _ = session_id;
        Ok(())
    }

    async fn admit_member_retire_for_destroy(
        &mut self,
        entry: &RosterEntry,
    ) -> Result<(), MobError> {
        let agent_identity = &entry.agent_identity;
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
        let session_id_for_route = self
            .dsl_authority
            .state()
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();
        let releasing = self
            .dsl_authority
            .state()
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();
        if self
            .dsl_authority
            .state()
            .pending_session_ingress_detach_runtime_ids
            .contains(&dsl_runtime_id)
        {
            let transition = self.apply_dsl_input_collect_transition(
                mob_dsl::MobMachineInput::RequestPendingSessionIngressDetachForMobDestroy {
                    mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                    agent_runtime_id: dsl_runtime_id.clone(),
                },
                "destroy_request_pending_session_ingress_detach",
            )?;
            let obligations =
                crate::generated::protocol_mob_destroying_session_ingress::extract_obligations(
                    &transition,
                );
            let obligation = match obligations.as_slice() {
                [obligation] => obligation.clone(),
                [] => {
                    return Err(MobError::Internal(format!(
                        "MobMachine pending destroy detach for member '{agent_identity}' produced no generated session ingress obligation"
                    )));
                }
                _ => {
                    return Err(MobError::Internal(format!(
                        "MobMachine pending destroy detach for member '{agent_identity}' produced multiple generated session ingress obligations"
                    )));
                }
            };
            if obligation.mob_id() != &mob_dsl::MobId::from_domain(&self.definition.id)
                || obligation.agent_runtime_id() != &dsl_runtime_id
            {
                return Err(MobError::Internal(format!(
                    "MobMachine pending destroy detach for member '{agent_identity}' generated an obligation for a different mob/runtime"
                )));
            }
            if let Some(detach_session_id) =
                Self::destroy_ingress_detach_session_id(entry, releasing.as_ref())?
            {
                self.detach_session_ingress_for_mob_destroy(&detach_session_id, obligation)
                    .await?;
            } else {
                self.acknowledge_absent_session_ingress_for_mob_destroy(
                    agent_identity,
                    obligation,
                )?;
            }
            return self.flush_routed_effects().await;
        }

        self.apply_dsl_signal(
            mob_dsl::MobMachineSignal::AdmitDestroyMemberRetire {
                agent_identity: dsl_identity,
                agent_runtime_id: dsl_runtime_id,
                fence_token: mob_dsl::FenceToken::from_domain(entry.fence_token),
                session_id: session_id_for_route,
            },
            "destroy_mark_member_retiring",
        )?;
        self.flush_routed_effects().await
    }

    async fn handle_retire_inner(
        &mut self,
        agent_identity: &MeerkatId,
        bulk: bool,
        preserve_realtime_binding: bool,
    ) -> Result<(), MobError> {
        tracing::debug!(
            agent_identity = %agent_identity,
            bulk,
            preserve_realtime_binding,
            "MobActor::handle_retire_inner start"
        );
        // Idempotent: already retired / never existed is success.
        let entry = {
            let roster = self.roster.read().await;
            roster.get(agent_identity).cloned()
        };
        let Some(entry) = entry else {
            self.apply_command_admission(
                mob_dsl::MobMachineInput::RetireAbsent {
                    agent_identity: mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(
                        agent_identity.as_str(),
                    )),
                },
                MobState::Running,
                "handle_retire_inner_absent",
            )?;
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                "retire requested for unknown meerkat id; MobMachine accepted RetireAbsent"
            );
            return Ok(());
        };
        tracing::debug!(
            agent_identity = %agent_identity,
            member_ref = ?entry.member_ref,
            runtime_id = %entry.agent_runtime_id,
            "MobActor::handle_retire_inner loaded roster entry"
        );

        // Mark as Retiring in the DSL (blocks re-spawn with same ID).
        // Shell roster does not carry authoritative state; `member_state_markers`
        // in the DSL is the source of truth and overlays the read-only projection
        // on snapshot construction.
        //
        // The DSL guards reject Retire when the runtime_id is absent from
        // `live_runtime_ids` or the phase forbids it.
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let detach_session_id = if preserve_realtime_binding {
            None
        } else {
            self.machine_bridge_session_id_for_identity(&domain_identity)
        };
        let releasing = if preserve_realtime_binding {
            None
        } else {
            self.dsl_authority
                .state()
                .member_session_bindings
                .get(&dsl_identity)
                .cloned()
        };
        let session_id_for_route = releasing.clone().or_else(|| {
            self.dsl_authority
                .state()
                .member_session_bindings
                .get(&dsl_identity)
                .cloned()
        });
        let retire_input = mob_dsl::MobMachineInput::Retire {
            mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
            agent_identity: dsl_identity,
            generation: mob_dsl::Generation::from_domain(entry.generation),
            releasing,
            session_id: session_id_for_route.clone(),
        };

        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_retire_inner checking prior retire event"
        );
        let retire_event_already_present = self
            .retire_event_exists(agent_identity, &entry.member_ref)
            .await?;
        tracing::debug!(
            agent_identity = %agent_identity,
            retire_event_already_present,
            "MobActor::handle_retire_inner checked prior retire event"
        );
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_retire_inner planning trust cleanup"
        );
        let trust_cleanup_plan = self
            .member_retire_trust_cleanup_plan(agent_identity, &entry)
            .await?;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_retire_inner planned trust cleanup"
        );
        let preserve_topology_for_respawn = preserve_realtime_binding;
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
        let cleanup_retry = retire_event_already_present
            || entry.state == crate::roster::MemberState::Retiring
            || matches!(
                self.dsl_authority
                    .state()
                    .member_state_markers
                    .get(&dsl_runtime_id),
                Some(mob_dsl::MobMemberState::Retiring)
            );

        if cleanup_retry {
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                "retrying member retire cleanup from retained roster anchor"
            );
            // The generated MobMachine already owns the Retiring marker; the
            // roster remains a projection and is materialized through the
            // machine lifecycle projection on read.
        } else {
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_retire_inner preparing Retire transition"
            );
            let prepared_retire = self.prepare_dsl_input_transition(
                retire_input.clone(),
                "handle_retire_inner_mark_retiring",
            )?;
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_retire_inner prepared Retire transition"
            );
            Self::require_member_lifecycle_journal_effect(
                &prepared_retire.transition,
                mob_dsl::MobLifecycleJournalKind::MemberRetired,
                &entry.agent_identity,
                &entry.agent_runtime_id,
                None,
                entry.generation,
                session_id_for_route.clone(),
                "handle_retire_inner_mark_retiring",
            )?;
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_retire_inner validated Retire transition journal"
            );

            if !preserve_topology_for_respawn {
                tracing::debug!(
                    agent_identity = %agent_identity,
                    "MobActor::handle_retire_inner cleaning external peer edges"
                );
                self.cleanup_retiring_external_peer_edges(&domain_identity)
                    .await?;
                tracing::debug!(
                    agent_identity = %agent_identity,
                    "MobActor::handle_retire_inner cleaned external peer edges"
                );
            }

            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_retire_inner preparing Retire transition after external cleanup"
            );
            let prepared_retire = self.prepare_dsl_input_transition(
                retire_input,
                "handle_retire_inner_mark_retiring_after_external_cleanup",
            )?;
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_retire_inner prepared Retire transition after external cleanup"
            );
            Self::require_member_lifecycle_journal_effect(
                &prepared_retire.transition,
                mob_dsl::MobLifecycleJournalKind::MemberRetired,
                &entry.agent_identity,
                &entry.agent_runtime_id,
                None,
                entry.generation,
                session_id_for_route.clone(),
                "handle_retire_inner_mark_retiring_after_external_cleanup",
            )?;
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_retire_inner validated post-cleanup Retire journal"
            );
            if !retire_event_already_present
                && let Err(error) = self.append_retire_event_for_entry(&entry).await
            {
                return Err(error);
            }
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_retire_inner appended retire event"
            );

            let mut detach_obligations =
                crate::generated::protocol_mob_destroying_session_ingress::extract_obligations(
                    &prepared_retire.transition,
                );
            self.commit_prepared_dsl_transition(prepared_retire)?;
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_retire_inner committed Retire transition"
            );
            if let (Some(obligation), Some(session_id)) =
                (detach_obligations.pop(), detach_session_id)
            {
                self.detach_session_ingress_for_mob_destroy(&session_id, obligation)
                    .await?;
            }

            // Flush session-backed routed effects before the disposal pipeline
            // tears down the runtime session. If the dispatch target has already
            // disappeared, drop the stale per-session effect so the actor returns
            // a typed lifecycle result instead of crashing later.
            if let Err(error) = self.flush_routed_effects().await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %agent_identity,
                    %error,
                    "pre-disposal routed-effect flush failed; proceeding with disposal"
                );
                if let Some(session_id) = entry.member_ref.bridge_session_id() {
                    self.discard_pending_routed_effects_for_session(session_id);
                }
            }
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_retire_inner flushed routed effects"
            );
        }

        // Snapshot context and run disposal pipeline.
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_retire_inner building disposal context"
        );
        let ctx = self
            .disposal_context_from_entry(agent_identity, &entry, trust_cleanup_plan)
            .await;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_retire_inner built disposal context"
        );
        let mut policy: Box<dyn ErrorPolicy> = if bulk {
            Box::new(BulkBestEffort)
        } else {
            Box::new(WarnAndContinue)
        };
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_retire_inner disposing member"
        );
        let report = self.dispose_member(&ctx, policy.as_mut()).await;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_retire_inner disposed member"
        );

        // Once generated Retire has accepted, machine-owned topology cleanup is
        // independent from best-effort shell notification/trust cleanup.
        if !preserve_topology_for_respawn {
            self.cleanup_retired_member_machine_wiring(&ctx)?;
        }

        // ArchiveSession is critical: a skipped archive means an orphan session
        // the caller believes was cleaned up. Surface the error.
        // Comms steps (NotifyPeers, RemoveTrustEdges) remain best-effort.
        if let Some((_, error)) = report
            .skipped
            .iter()
            .find(|(step, _)| *step == DisposalStep::ArchiveSession)
        {
            return Err(MobError::Internal(format!(
                "disposal completed but ArchiveSession failed: {error}"
            )));
        }
        if let Some((step, error)) = &report.aborted_at
            && *step == DisposalStep::ArchiveSession
        {
            return Err(MobError::Internal(format!(
                "disposal aborted at ArchiveSession: {error}"
            )));
        }

        self.delete_external_binding_overlay_for_member(&entry.agent_identity, entry.generation)
            .await?;

        Ok(())
    }

    /// Respawn a member: retire the old session and spawn a fresh one with the
    /// same identity, profile, wiring, and labels. The old session is archived;
    /// the new session gets a fresh session ID.
    ///
    /// This is helper composition over primitive mob behavior. No rollback is
    /// attempted after retire. Returns a receipt on full success, or a
    /// structured error on failure.
    async fn handle_respawn(
        &mut self,
        agent_identity: MeerkatId,
        initial_message: Option<ContentInput>,
    ) -> Result<super::handle::MemberRespawnReceipt, super::handle::MobRespawnError> {
        use super::handle::{MemberRespawnReceipt, MobRespawnError};

        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_respawn start"
        );
        self.ensure_pending_spawn_alignment("handle_respawn preflight")
            .map_err(MobRespawnError::from)?;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_respawn preflight aligned"
        );

        let canceled = self
            .cancel_pending_spawns_for_member(
                &agent_identity,
                "respawn command superseded pending spawn",
            )
            .await
            .map_err(MobRespawnError::from)?;
        if canceled > 0 {
            tracing::info!(
                agent_identity = %agent_identity,
                canceled,
                "respawn canceled pending spawn lineage before replacement workflow"
            );
        }
        self.cancel_peer_deliveries_for_member(&agent_identity, "member is respawning")
            .await;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_respawn canceled peer deliveries"
        );

        // 1. Snapshot all replacement inputs before retiring. Topology comes
        // from the MobMachine authority; the roster is only the read model
        // that supplies profile/runtime binding details for the old member.
        let snapshot = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(&agent_identity)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(agent_identity.clone()))?;
            let restore_wiring = self
                .machine_restore_wiring_plan(&agent_identity)
                .map_err(MobRespawnError::from)?;
            let binding = match &entry.member_ref {
                crate::event::MemberRef::BackendPeer {
                    peer_id,
                    address,
                    pubkey,
                    bootstrap_token,
                    ..
                } => crate::RuntimeBinding::External {
                    peer_id: peer_id.clone(),
                    address: address.clone(),
                    bootstrap_token: bootstrap_token.clone(),
                    pubkey: *pubkey,
                },
                crate::event::MemberRef::Session { .. } => crate::RuntimeBinding::Session,
            };
            let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
            let cleanup_retry = entry.state == crate::roster::MemberState::Retiring
                || matches!(
                    self.dsl_authority
                        .state()
                        .member_state_markers
                        .get(&dsl_runtime_id),
                    Some(mob_dsl::MobMemberState::Retiring)
                );
            RespawnSnapshot {
                profile_name: entry.role.clone(),
                runtime_mode: entry.runtime_mode,
                labels: entry.labels.clone(),
                old_runtime_id: entry.agent_runtime_id.clone(),
                old_fence_token: entry.fence_token,
                generation: entry.generation,
                restore_wiring,
                binding,
                effective_profile_override: entry.effective_profile_override,
                cleanup_retry,
            }
        };
        tracing::debug!(
            agent_identity = %agent_identity,
            old_runtime_id = %snapshot.old_runtime_id,
            runtime_mode = ?snapshot.runtime_mode,
            cleanup_retry = snapshot.cleanup_retry,
            "MobActor::handle_respawn captured snapshot"
        );

        let original_identity = AgentIdentity::from(agent_identity.as_str());
        let mut replacement_spec = super::handle::SpawnMemberSpec::new(
            snapshot.profile_name.clone(),
            original_identity.clone(),
        );
        replacement_spec.initial_message = initial_message;
        replacement_spec.runtime_mode = Some(snapshot.runtime_mode);
        replacement_spec.binding = Some(snapshot.binding.clone());
        replacement_spec.labels = Some(snapshot.labels.clone());
        replacement_spec.override_profile = snapshot.effective_profile_override.clone();
        self.customize_spawn_spec(
            super::handle::SpawnSource::Respawn,
            None,
            &mut replacement_spec,
        )
        .map_err(MobRespawnError::from)?;
        if replacement_spec.identity != original_identity {
            return Err(MobRespawnError::from(MobError::Internal(format!(
                "spawn customizer cannot change respawn identity from '{original_identity}' to '{}'",
                replacement_spec.identity
            ))));
        }
        if replacement_spec.role_name != snapshot.profile_name {
            return Err(MobRespawnError::from(MobError::Internal(format!(
                "spawn customizer cannot change respawn profile for '{original_identity}' from '{}' to '{}'",
                snapshot.profile_name, replacement_spec.role_name
            ))));
        }
        if replacement_spec.binding.as_ref() != Some(&snapshot.binding) {
            return Err(MobRespawnError::from(MobError::Internal(format!(
                "spawn customizer cannot change respawn runtime binding for '{original_identity}'"
            ))));
        }
        let super::handle::SpawnMemberSpec {
            role_name: _,
            identity: _,
            initial_message: replacement_initial_message,
            runtime_mode: _,
            backend: _,
            binding: _,
            context: replacement_context,
            labels: replacement_labels,
            launch_mode: _,
            tool_access_policy: _tool_access_policy,
            budget_split_policy: _budget_split_policy,
            auto_wire_parent: _,
            additional_instructions: replacement_additional_instructions,
            shell_env: replacement_shell_env,
            inherited_tool_filter: replacement_inherited_tool_filter,
            override_profile: replacement_profile_override,
            auth_binding: replacement_auth_binding,
            external_tools: replacement_external_tools,
            system_prompt_override: replacement_system_prompt_override,
            continuity_intent: replacement_continuity_intent,
        } = replacement_spec;
        let replacement_labels = replacement_labels.unwrap_or_default();

        if !snapshot.cleanup_retry {
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_respawn previewing Respawn admission"
            );
            self.preview_dsl_input(
                mob_dsl::MobMachineInput::Respawn {
                    agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                        &snapshot.old_runtime_id,
                    ),
                },
                "handle_respawn_admission",
            )
            .map_err(|_| MobRespawnError::from(self.invalid_transition_to(MobState::Running)))?;
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_respawn previewed Respawn admission"
            );
        }
        #[cfg(feature = "runtime-adapter")]
        let respawn_peer_only_owner_context =
            if matches!(snapshot.binding, crate::RuntimeBinding::External { .. }) {
                Some(
                    self.generated_peer_only_operation_owner_context(
                        &original_identity,
                        &snapshot.binding,
                        "respawn_peer_only_operation_owner",
                    )
                    .await
                    .map_err(MobRespawnError::from)?,
                )
            } else {
                None
            };
        #[cfg(not(feature = "runtime-adapter"))]
        let respawn_peer_only_owner_context: Option<(
            SessionId,
            Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        )> = None;

        let replacement_generation = snapshot.generation.next();

        // 2. Retire the existing member (archives the session, removes from roster).
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_respawn retiring previous member"
        );
        if let Err(error) = self.handle_retire_inner(&agent_identity, false, true).await {
            let roster_still_contains_member = {
                let roster = self.roster.read().await;
                roster.get(&agent_identity).is_some()
            };
            if roster_still_contains_member {
                return Err(MobRespawnError::from(error));
            }
            let mut cleanup_report = super::handle::PreviousMemberCleanupReport {
                identity: AgentIdentity::from(agent_identity.as_str()),
                agent_runtime_id: snapshot.old_runtime_id.clone(),
                fence_token: snapshot.old_fence_token,
                retire_attempted: true,
                retire_error: Some(error.to_string()),
                confirmatory_observation_attempted: false,
                confirmatory_observation: None,
                destroy_attempted: false,
                destroy_error: None,
            };

            match &snapshot.binding {
                crate::RuntimeBinding::External { .. } => {
                    cleanup_report.confirmatory_observation_attempted = true;
                    match self
                        .observe_peer_only_binding(
                            &snapshot.binding,
                            std::time::Duration::from_millis(750),
                        )
                        .await
                    {
                        Ok(observation) => {
                            cleanup_report.confirmatory_observation =
                                Some(format!("state={}", observation.state));
                            let observation_is_terminal = self
                                .observation_is_terminal(&observation)
                                .map_err(MobRespawnError::from)?;
                            if !observation_is_terminal {
                                cleanup_report.destroy_attempted = true;
                                if let Err(destroy_error) = self
                                    .destroy_peer_only_binding(
                                        &snapshot.binding,
                                        std::time::Duration::from_secs(5),
                                    )
                                    .await
                                {
                                    cleanup_report.destroy_error = Some(destroy_error.to_string());
                                    return Err(MobRespawnError::PreviousMemberCleanupAmbiguous {
                                        report: cleanup_report,
                                    });
                                }
                            }
                        }
                        Err(observe_error) => {
                            cleanup_report.confirmatory_observation =
                                Some(observe_error.to_string());
                            cleanup_report.destroy_attempted = true;
                            if let Err(destroy_error) = self
                                .destroy_peer_only_binding(
                                    &snapshot.binding,
                                    std::time::Duration::from_secs(5),
                                )
                                .await
                            {
                                cleanup_report.destroy_error = Some(destroy_error.to_string());
                                return Err(MobRespawnError::PreviousMemberCleanupAmbiguous {
                                    report: cleanup_report,
                                });
                            }
                        }
                    }
                }
                crate::RuntimeBinding::Session => {
                    return Err(MobRespawnError::PreviousMemberCleanupAmbiguous {
                        report: cleanup_report,
                    });
                }
            }
        }
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_respawn retired previous member"
        );

        // 3. Rebuild the replacement spawn preserving identity, profile, labels, mode, and peer intent.
        let (prompt, initial_turn_prompt) = match replacement_initial_message {
            Some(message) => {
                let prompt = message;
                (prompt.clone(), Some(prompt))
            }
            None => (
                ContentInput::from(
                    self.fallback_spawn_prompt(&snapshot.profile_name, &agent_identity),
                ),
                None,
            ),
        };
        // Prefer roster's effective_profile_override on respawn for lifecycle safety.
        let mut profile = if let Some(p) = replacement_profile_override.clone() {
            p
        } else {
            self.definition
                .resolve_profile(&snapshot.profile_name, self.realm_profile_store.as_ref())
                .await?
        };
        if replacement_inherited_tool_filter.is_some() && replacement_profile_override.is_none() {
            build::open_profile_tool_categories_for_inherited_filter(&mut profile);
        }
        let replacement_authorized_profile_material = self
            .authorize_spawn_profile_material(
                &agent_identity,
                &snapshot.profile_name,
                &profile,
                "respawn_profile_authority",
            )
            .map_err(MobRespawnError::from)?;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_respawn authorized replacement profile material"
        );
        let external_tools =
            self.external_tools_for_profile(&profile, replacement_external_tools)?;
        let mut config = build::build_agent_config(build::BuildAgentConfigParams {
            mob_id: &self.definition.id,
            profile_name: &snapshot.profile_name,
            agent_identity: &agent_identity,
            profile: &profile,
            definition: &self.definition,
            external_tools,
            context: replacement_context,
            labels: Some(replacement_labels.clone()),
            additional_instructions: replacement_additional_instructions,
            shell_env: replacement_shell_env,
            mob_tool_authority_context: None,
            inherited_tool_filter: replacement_inherited_tool_filter,
            system_prompt_override: replacement_system_prompt_override,
        })
        .await?;
        config.keep_alive = snapshot.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
        if let Some(ref client) = self.default_llm_client {
            config.llm_client_override = Some(client.clone());
        }
        if let Some(ref cref) = replacement_auth_binding {
            config.auth_binding = Some(cref.clone());
        }
        let req = build::to_create_session_request(&config, prompt.clone());
        let peer_name = format!(
            "{}/{}/{}",
            self.definition.id, snapshot.profile_name, agent_identity
        );
        let mut provision_request = ProvisionMemberRequest {
            create_session: req,
            binding: snapshot.binding.clone(),
            peer_name,
            owner_bridge_session_id: None,
            ops_registry: None,
            generated_self_owned_operation_owner: None,
        };
        if let Some((owner_bridge_session_id, ops_registry)) = respawn_peer_only_owner_context {
            provision_request.owner_bridge_session_id = Some(owner_bridge_session_id);
            provision_request.ops_registry = Some(ops_registry);
        }
        let admitted_bridge_session_id =
            admit_bridge_session_for_spawn(&mut provision_request.create_session);
        tracing::debug!(
            agent_identity = %agent_identity,
            bridge_session_id = %admitted_bridge_session_id,
            "MobActor::handle_respawn admitted replacement bridge session"
        );

        let respawn_spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        let generated_self_owned_operation_owner = self
            .stage_orchestrator_spawn(&agent_identity, &admitted_bridge_session_id)
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(agent_identity.as_str()),
                reason: format!("failed to stage respawn replacement spawn: {error}"),
            })?;
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::handle_respawn staged replacement spawn"
        );
        Self::apply_generated_self_owned_operation_owner(
            &mut provision_request,
            generated_self_owned_operation_owner,
        )
        .map_err(|error| MobRespawnError::SpawnAfterRetire {
            identity: AgentIdentity::from(agent_identity.as_str()),
            reason: format!("failed to authorize respawn replacement operation owner: {error}"),
        })?;
        let (respawn_inline_reply_tx, _respawn_inline_reply_rx) = oneshot::channel();
        let respawn_pending = PendingSpawn {
            profile_name: snapshot.profile_name.clone(),
            agent_identity: agent_identity.clone(),
            admitted_bridge_session_id,
            prompt: prompt.clone(),
            initial_turn_prompt: initial_turn_prompt.clone(),
            runtime_mode: snapshot.runtime_mode,
            labels: replacement_labels.clone(),
            owner_bridge_session_id: None,
            auto_wire_parent: false,
            restore_wiring: (!snapshot.restore_wiring.local_peers.is_empty()
                || !snapshot.restore_wiring.external_peers.is_empty())
            .then_some(snapshot.restore_wiring.clone()),
            effective_profile_override: replacement_profile_override.clone(),
            authorized_profile_material: replacement_authorized_profile_material.clone(),
            continuity_intent: replacement_continuity_intent.clone(),
            progress: Arc::new(std::sync::Mutex::new(PendingSpawnProgress::default())),
            reply_tx: respawn_inline_reply_tx,
        };
        let respawn_inline_task = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        self.insert_pending_spawn(respawn_spawn_ticket, respawn_pending, respawn_inline_task);
        if let Err(error) = self.ensure_pending_spawn_alignment("handle_respawn staged replacement")
        {
            tracing::error!(
                agent_identity = %agent_identity,
                error = %error,
                "pending spawn alignment violated while staging respawn replacement"
            );
            if let Err(cleanup_error) = self
                .fail_all_pending_spawns(
                    "pending spawn alignment violated while staging respawn replacement",
                )
                .await
            {
                return Err(MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(agent_identity.as_str()),
                    reason: cleanup_error.to_string(),
                });
            }
            return Err(MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(agent_identity.as_str()),
                reason: error.to_string(),
            });
        }

        // 4. Provision and finalize the replacement member inline so the receipt reflects
        //    the committed canonical member/session state before we return.
        let replacement_result: Result<super::handle::MemberSpawnReceipt, MobRespawnError> = Box::pin(async {
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_respawn provisioning replacement member"
            );
            let spawn_receipt = self
                .provisioner
                .provision_member(provision_request)
                .await
                .map_err(|error| MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(agent_identity.as_str()),
                    reason: error.to_string(),
                })?;
            tracing::debug!(
                agent_identity = %agent_identity,
                member_ref = ?spawn_receipt.member_ref,
                "MobActor::handle_respawn provisioned replacement member"
            );
            if snapshot.runtime_mode == crate::MobRuntimeMode::AutonomousHost
                && let Err(capability_error) =
                    Self::ensure_autonomous_dispatch_capability_for_provisioner(
                        &self.provisioner,
                        &agent_identity,
                        &spawn_receipt.member_ref,
                    )
                    .await
            {
                if let Err(retire_error) = self
                    .provisioner
                    .retire_member(&spawn_receipt.member_ref)
                    .await
                {
                    return Err(MobRespawnError::SpawnAfterRetire {
                        identity: AgentIdentity::from(agent_identity.as_str()),
                        reason: format!(
                            "autonomous capability check failed: {capability_error}; cleanup retire failed: {retire_error}"
                        ),
                    });
                }
                return Err(MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(agent_identity.as_str()),
                    reason: capability_error.to_string(),
                });
            }

            let provision = PendingProvision::new(
                spawn_receipt.member_ref.clone(),
                agent_identity.clone(),
                self.provisioner.clone(),
            );
            if let Err(error) = self.require_state(&[MobState::Running]) {
                if let Err(retire_error) = provision.rollback().await {
                    return Err(MobRespawnError::SpawnAfterRetire {
                        identity: AgentIdentity::from(agent_identity.as_str()),
                        reason: format!(
                            "mob state changed before respawn finalization: {error}; cleanup retire failed: {retire_error}"
                        ),
                    });
                }
                return Err(MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(agent_identity.as_str()),
                    reason: error.to_string(),
                });
            }

            if !snapshot.restore_wiring.local_peers.is_empty()
                || !snapshot.restore_wiring.external_peers.is_empty()
            {
                tracing::info!(
                    agent_identity = %agent_identity,
                    local_peers = ?snapshot.restore_wiring.local_peers,
                    external_peers = ?snapshot.restore_wiring.external_peers,
                    "respawn: restoring peer wiring during replacement finalization"
                );
            }

            let respawn_fence = self.issue_fence_token();
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_respawn finalizing replacement spawn"
            );
            let finalized = self
                .finalize_spawn_from_pending(
                    &snapshot.profile_name,
                    &agent_identity,
                    replacement_generation,
                    respawn_fence,
                    snapshot.runtime_mode,
                    prompt,
                    initial_turn_prompt,
                    replacement_labels,
                    provision,
                    spawn_receipt.operation_id,
                    None,
                    false,
                    (!snapshot.restore_wiring.local_peers.is_empty()
                        || !snapshot.restore_wiring.external_peers.is_empty())
                    .then_some(snapshot.restore_wiring.clone()),
                    replacement_profile_override,
                    replacement_authorized_profile_material,
                    replacement_continuity_intent,
                )
                .await
                .map_err(|error| MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(agent_identity.as_str()),
                reason: error.to_string(),
            })?;
            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_respawn finalized replacement spawn"
            );

            tracing::debug!(
                agent_identity = %agent_identity,
                "MobActor::handle_respawn resolving topology restore"
            );
            let topology_restore = self
                .resolve_respawn_topology_restore_result(
                    &agent_identity,
                    finalized.failed_restore_peer_ids,
                )
                .map_err(|error| MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(agent_identity.as_str()),
                    reason: error.to_string(),
                })?;
            tracing::debug!(
                agent_identity = %agent_identity,
                result = ?topology_restore.result,
                "MobActor::handle_respawn resolved topology restore"
            );
            match topology_restore.result {
                mob_dsl::RespawnTopologyRestoreResultKind::Completed => Ok(finalized.receipt),
                mob_dsl::RespawnTopologyRestoreResultKind::TopologyRestoreFailed => {
                    Err(MobRespawnError::TopologyRestoreFailed {
                        receipt: super::handle::MemberRespawnReceipt::new(
                            AgentIdentity::from(agent_identity.as_str()),
                            crate::ids::AgentRuntimeId::new(
                                AgentIdentity::from(agent_identity.as_str()),
                                replacement_generation,
                            ),
                            snapshot.old_fence_token,
                            respawn_fence,
                        ),
                        failed_peer_ids: topology_restore.failed_peer_ids,
                    })
                }
            }
        })
        .await;

        let (_respawn_pending, respawn_task) =
            self.complete_pending_spawn_slot(respawn_spawn_ticket, "respawn replacement spawn");
        if let Some(handle) = respawn_task {
            handle.abort();
        }
        self.ensure_pending_spawn_alignment("handle_respawn completion")
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(agent_identity.as_str()),
                reason: error.to_string(),
            })?;
        let _replacement = replacement_result?;

        // 5. Build the receipt from the committed replacement member reference.
        Ok(MemberRespawnReceipt::new(
            AgentIdentity::from(agent_identity.as_str()),
            crate::ids::AgentRuntimeId::new(
                AgentIdentity::from(agent_identity.as_str()),
                replacement_generation,
            ),
            snapshot.old_fence_token,
            self.roster
                .read()
                .await
                .get(&agent_identity)
                .map(|entry| entry.fence_token)
                .unwrap_or(snapshot.old_fence_token),
        ))
    }

    // -----------------------------------------------------------------------
    // Disposal pipeline
    // -----------------------------------------------------------------------

    fn machine_wired_peer_identities_for(
        &self,
        identity: &AgentIdentity,
    ) -> BTreeSet<AgentIdentity> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        let state = self.dsl_authority.state();
        let mut wired_to = self.machine_member_wired_peer_identities_for(identity);
        wired_to.extend(
            state
                .external_peer_edges
                .iter()
                .filter(|edge| edge.local == dsl_identity)
                .map(|edge| AgentIdentity::from(edge.endpoint.name.0.as_str())),
        );
        wired_to
    }

    fn machine_member_wired_peer_identities_for(
        &self,
        identity: &AgentIdentity,
    ) -> BTreeSet<AgentIdentity> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        self.dsl_authority
            .state()
            .wiring_edges
            .iter()
            .filter_map(|edge| {
                if edge.a == dsl_identity {
                    Some(AgentIdentity::from(edge.b.0.as_str()))
                } else if edge.b == dsl_identity {
                    Some(AgentIdentity::from(edge.a.0.as_str()))
                } else {
                    None
                }
            })
            .collect()
    }

    fn machine_external_peer_edges_for(
        &self,
        identity: &AgentIdentity,
    ) -> Vec<mob_dsl::ExternalPeerEdge> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        self.dsl_authority
            .state()
            .external_peer_edges
            .iter()
            .filter(|edge| edge.local == dsl_identity)
            .cloned()
            .collect()
    }

    async fn cleanup_retiring_external_peer_edges(
        &mut self,
        retiring_identity: &AgentIdentity,
    ) -> Result<(), MobError> {
        let local = MeerkatId::from(retiring_identity.as_str());
        for edge in self.machine_external_peer_edges_for(retiring_identity) {
            let peer_name = meerkat_core::comms::PeerName::new(edge.endpoint.name.0.clone())
                .map_err(|error| {
                    MobError::WiringError(format!(
                        "retire external cleanup has invalid peer name '{}': {error}",
                        edge.endpoint.name.0
                    ))
                })?;
            let stale_cleanup_spec =
                Self::trusted_peer_descriptor_from_machine_endpoint(&edge.endpoint)?;
            self.handle_unwire_external(local.clone(), peer_name, Some(stale_cleanup_spec))
                .await?;
        }
        Ok(())
    }

    async fn member_retire_trust_cleanup_plan(
        &mut self,
        agent_identity: &MeerkatId,
        entry: &RosterEntry,
    ) -> Result<RetireTrustCleanupPlan, MobError> {
        let retiring_identity = AgentIdentity::from(agent_identity.as_str());
        let machine_wired_peer_identities =
            self.machine_member_wired_peer_identities_for(&retiring_identity);
        if machine_wired_peer_identities.is_empty() {
            return Ok(RetireTrustCleanupPlan::empty());
        }
        let (retiring_spec, retiring_comms) = match self
            .resolve_wiring_endpoint(entry, "retire trust authority retiring member")
            .await
        {
            Ok(WiringEndpoint::Local { spec, comms, .. }) => (spec, Some(comms)),
            Ok(WiringEndpoint::PeerOnly { spec, .. }) => {
                (spec, Some(self.supervisor_bridge.runtime_core().await))
            }
            Err(error) => {
                let retained_spec = match self.machine_member_peer_spec_for(
                    &retiring_identity,
                    "retire trust authority retiring member",
                )? {
                    Some(spec) => Some(spec),
                    None => match self.roster_member_peer_spec_for(
                        entry,
                        "retire trust authority retiring member",
                    )? {
                        Some(spec) => Some(spec),
                        None => {
                            self.retained_member_peer_spec_from_wired_peer_trust(
                                entry,
                                &machine_wired_peer_identities,
                                "retire trust authority retiring member",
                            )
                            .await?
                        }
                    },
                };
                match retained_spec {
                    Some(spec) => {
                        tracing::debug!(
                            mob_id = %self.definition.id,
                            agent_identity = %retiring_identity,
                            "retire trust authority using retained generated peer endpoint for member without live comms runtime"
                        );
                        (spec, None)
                    }
                    None => {
                        tracing::debug!(
                            mob_id = %self.definition.id,
                            agent_identity = %retiring_identity,
                            error = %error,
                            "retire trust authority found no retained peer trust to clean up for member without live comms runtime"
                        );
                        return Ok(RetireTrustCleanupPlan {
                            retiring_comms: None,
                            retiring_spec: None,
                            machine_wired_peer_identities,
                            trust_unwire_authority_by_peer: BTreeMap::new(),
                        });
                    }
                }
            }
        };
        let retiring_peer_id = Self::trusted_peer_removal_key(&retiring_spec);
        let mut authorities = BTreeMap::new();
        for peer_identity in &machine_wired_peer_identities {
            let peer_entry = {
                let roster = self.roster.read().await;
                roster.get_by_identity(peer_identity).cloned()
            };
            let Some(peer_entry) = peer_entry else {
                continue;
            };
            let peer_spec = match self
                .resolve_wiring_endpoint(&peer_entry, "member_retire_trust_authority peer")
                .await
            {
                Ok(WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. }) => {
                    spec
                }
                Err(error) => {
                    let retained_spec = match self.machine_member_peer_spec_for(
                        peer_identity,
                        "member_retire_trust_authority peer",
                    )? {
                        Some(spec) => Some(spec),
                        None => self.roster_member_peer_spec_for(
                            &peer_entry,
                            "member_retire_trust_authority peer",
                        )?,
                    };
                    retained_spec.ok_or(error)?
                }
            };
            let peer_peer_id = Self::trusted_peer_removal_key(&peer_spec);
            let peer_identity_for_cleanup = peer_identity.clone();
            let edge = mob_dsl::WiringEdge::new(
                mob_dsl::AgentIdentity::from_domain(&retiring_identity),
                mob_dsl::AgentIdentity::from_domain(peer_identity),
            );
            let handoff = match self.authorize_member_trust_unwiring(
                &edge,
                "member_retire_trust_authority",
            ) {
                Ok(handoff) => handoff,
                Err(unwiring_error) => self
                    .authorize_member_trust_cleanup_observed(
                        &edge,
                        &retiring_identity,
                        &retiring_peer_id,
                        &peer_identity_for_cleanup,
                        &peer_peer_id,
                        "member_retire_trust_authority_observed",
                    )
                    .map_err(|cleanup_error| {
                        MobError::WiringError(format!(
                            "member retire trust cleanup failed: {unwiring_error}; observed cleanup failed: {cleanup_error}"
                        ))
                    })?,
            };
            let authority =
                handoff.unwiring_authority_for(&retiring_identity, &retiring_peer_id)?;
            authorities.insert(peer_identity.clone(), authority);
        }
        Ok(RetireTrustCleanupPlan {
            retiring_comms,
            retiring_spec: Some(retiring_spec),
            machine_wired_peer_identities,
            trust_unwire_authority_by_peer: authorities,
        })
    }

    /// Snapshot member state for disposal from a roster entry.
    async fn disposal_context_from_entry(
        &self,
        agent_identity: &MeerkatId,
        entry: &RosterEntry,
        trust_cleanup_plan: RetireTrustCleanupPlan,
    ) -> DisposalContext {
        let retiring_key = self
            .provisioner_comms(&entry.member_ref)
            .await
            .and_then(|comms| comms.public_key());
        DisposalContext {
            agent_identity: agent_identity.clone(),
            entry: entry.clone(),
            retiring_key,
            retiring_comms: trust_cleanup_plan.retiring_comms,
            retiring_spec: trust_cleanup_plan.retiring_spec,
            machine_wired_peer_identities: trust_cleanup_plan.machine_wired_peer_identities,
            trust_unwire_authority_by_peer: trust_cleanup_plan.trust_unwire_authority_by_peer,
        }
    }

    /// Execute the disposal pipeline for a member.
    ///
    /// Runs policy-driven steps in order, then unconditionally removes the
    /// member from the roster and prunes wire edge locks. The finally block
    /// runs regardless of whether the policy aborted.
    async fn dispose_member(
        &mut self,
        ctx: &DisposalContext,
        policy: &mut dyn ErrorPolicy,
    ) -> DisposalReport {
        let mut report = DisposalReport::new();

        for &step in &DisposalStep::ORDERED {
            tracing::info!(
                mob_id = %self.definition.id,
                agent_identity = %ctx.agent_identity,
                step = %step,
                "MobActor::dispose_member executing step"
            );
            match self.execute_step(step, ctx).await {
                Ok(()) => {
                    tracing::info!(
                        mob_id = %self.definition.id,
                        agent_identity = %ctx.agent_identity,
                        step = %step,
                        "MobActor::dispose_member completed step"
                    );
                    report.completed.push(step);
                }
                Err(error) => {
                    tracing::info!(
                        mob_id = %self.definition.id,
                        agent_identity = %ctx.agent_identity,
                        step = %step,
                        error = %error,
                        "MobActor::dispose_member step failed"
                    );
                    if policy.on_step_error(step, &error, ctx) {
                        report.skipped.push((step, error));
                    } else {
                        report.aborted_at = Some((step, error));
                        break;
                    }
                }
            }
        }

        if report.completed.contains(&DisposalStep::ArchiveSession)
            && let Err(error) = self.observe_member_retirement_archived(ctx)
        {
            report.aborted_at = Some((DisposalStep::ArchiveSession, error));
        }

        let archive_failed = report
            .skipped
            .iter()
            .any(|(step, _)| *step == DisposalStep::ArchiveSession)
            || matches!(
                report.aborted_at.as_ref(),
                Some((DisposalStep::ArchiveSession, _))
            );

        // Finally: edge-lock cleanup is unconditional, but roster removal is
        // gated on critical archive success so retry has a concrete member
        // anchor to operate on.
        self.dispose_prune_edge_locks(ctx).await;
        if archive_failed {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %ctx.agent_identity,
                "retaining retiring roster entry after ArchiveSession failure for retry"
            );
        } else {
            self.dispose_remove_from_roster(ctx).await;
            report.roster_removed = true;
        }
        report
    }

    /// Destroy cleanup must keep canonical member/session authority until all
    /// critical per-member cleanup steps succeed. General member disposal
    /// removes roster state in a finally block, but destroy retries rebuild
    /// cleanup work from the roster after a partial attempt.
    async fn dispose_member_for_destroy(&mut self, ctx: &DisposalContext) -> DisposalReport {
        let mut report = DisposalReport::new();
        let mut policy = AbortOnError;

        for &step in &DisposalStep::ORDERED {
            match self.execute_destroy_step(step, ctx).await {
                Ok(()) => report.completed.push(step),
                Err(error) => {
                    if policy.on_step_error(step, &error, ctx) {
                        report.skipped.push((step, error));
                    } else {
                        report.aborted_at = Some((step, error));
                        break;
                    }
                }
            }
        }
        if report.completed.contains(&DisposalStep::ArchiveSession)
            && let Err(error) = self.record_destroy_member_retirement_archived(ctx).await
        {
            report.aborted_at = Some((DisposalStep::ArchiveSession, error));
        }
        report
    }

    async fn execute_destroy_step(
        &mut self,
        step: DisposalStep,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        match step {
            DisposalStep::StopHostLoop => self.dispose_stop_host_loop_for_destroy(ctx).await,
            _ => self.execute_step(step, ctx).await,
        }
    }

    fn destroy_disposal_failure(report: &DisposalReport) -> Option<String> {
        if let Some((step, error)) = &report.aborted_at {
            return Some(format!("disposal aborted at {step}: {error}"));
        }
        report
            .skipped
            .first()
            .map(|(step, error)| format!("disposal completed but {step} failed: {error}"))
    }

    fn cleanup_retired_member_machine_wiring(
        &mut self,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        if ctx.machine_wired_peer_identities.is_empty() {
            return Ok(());
        }
        let retiring_identity = mob_dsl::AgentIdentity::from_domain(&ctx.entry.agent_identity);
        let cleanup_inputs = ctx
            .machine_wired_peer_identities
            .iter()
            .map(|peer_identity| mob_dsl::MobMachineInput::UnwireMembers {
                edge: mob_dsl::WiringEdge::new(
                    retiring_identity.clone(),
                    mob_dsl::AgentIdentity::from_domain(peer_identity),
                ),
            })
            .collect::<Vec<_>>();
        let prepared =
            self.prepare_dsl_inputs(&cleanup_inputs, "member_retire_machine_wiring_cleanup")?;
        self.commit_prepared_dsl_input(prepared)?;
        Ok(())
    }

    fn cleanup_member_machine_wiring_edge(
        &mut self,
        a: &AgentIdentity,
        b: &AgentIdentity,
        context: &'static str,
    ) -> Result<(), MobError> {
        let input = mob_dsl::MobMachineInput::UnwireMembers {
            edge: mob_dsl::WiringEdge::new(
                mob_dsl::AgentIdentity::from_domain(a),
                mob_dsl::AgentIdentity::from_domain(b),
            ),
        };
        let prepared = self.prepare_dsl_input_transition(input, context)?;
        self.commit_prepared_dsl_transition(prepared)?;
        Ok(())
    }

    /// Dispatch a disposal step. Exhaustive match ensures compiler forces new
    /// arms when `DisposalStep` variants are added.
    async fn execute_step(
        &mut self,
        step: DisposalStep,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        match step {
            DisposalStep::StopHostLoop => self.dispose_stop_host_loop(ctx).await,
            DisposalStep::NotifyPeers => self.dispose_notify_peers(ctx).await,
            DisposalStep::ArchiveSession => self.dispose_archive_session(ctx).await,
        }
    }

    /// Stop the autonomous member host loop. Session unregister is owned by
    /// the archive step after runtime retire/drain quiesces.
    async fn dispose_stop_host_loop(&mut self, ctx: &DisposalContext) -> Result<(), MobError> {
        if ctx.entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            self.stop_autonomous_member(&ctx.agent_identity, &ctx.entry.member_ref)
                .await?;
        }
        Ok(())
    }

    async fn dispose_stop_host_loop_for_destroy(
        &mut self,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        if ctx.entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost {
            return Ok(());
        }
        #[cfg(feature = "runtime-adapter")]
        if let (Some(adapter), Some(session_id)) = (
            &self.runtime_adapter,
            ctx.entry.member_ref.bridge_session_id(),
        ) && !adapter.contains_session(session_id).await
        {
            // A prior partial destroy can stop and unregister the autonomous
            // runtime, then fail later at ArchiveSession. Retry must continue
            // from the retained roster anchor and reach ArchiveSession again.
            return Ok(());
        }
        self.dispose_stop_host_loop(ctx).await
    }

    /// Notify all machine-wired peers that this member is retiring.
    ///
    /// Iterates the MobMachine-owned wiring snapshot; skips absent peers.
    /// Returns the first error encountered, if any.
    async fn dispose_notify_peers(&mut self, ctx: &DisposalContext) -> Result<(), MobError> {
        if ctx.machine_wired_peer_identities.is_empty() {
            return Ok(());
        }
        let Some(retiring_spec) = ctx.retiring_spec.as_ref() else {
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %ctx.agent_identity,
                "dispose_notify_peers: skipping lifecycle notice and trust cleanup because retiring endpoint is absent"
            );
            return Ok(());
        };
        let peer_identities = ctx
            .machine_wired_peer_identities
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut local_jobs = Vec::new();
        let mut peer_only_jobs = Vec::new();
        for peer_identity in peer_identities {
            let peer_entry = {
                let roster = self.roster.read().await;
                roster.get_by_identity(&peer_identity).cloned()
            };
            let Some(peer_entry) = peer_entry else {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    agent_identity = %ctx.agent_identity,
                    peer_id = %peer_identity,
                    "dispose_notify_peers: skipping absent peer"
                );
                continue;
            };
            match self
                .resolve_wiring_endpoint(&peer_entry, "dispose_notify_peers")
                .await?
            {
                WiringEndpoint::Local { comms, spec, .. } => {
                    let Some(authority) = ctx
                        .trust_unwire_authority_by_peer
                        .get(&peer_identity)
                        .cloned()
                    else {
                        return Err(MobError::WiringError(format!(
                            "dispose_notify_peers missing generated retire trust handoff for '{peer_identity}'"
                        )));
                    };
                    local_jobs.push((peer_identity, spec, comms, authority));
                }
                WiringEndpoint::PeerOnly { spec, binding } => {
                    peer_only_jobs.push((peer_identity, spec, binding));
                }
            }
        }
        let actor = &*self;
        let retiring_key = Self::trusted_peer_removal_key(retiring_spec);
        let retiring_comms = ctx.retiring_comms.clone();
        let mut local_tasks = FuturesUnordered::new();
        for (peer_identity, recipient_spec, recipient_comms, authority) in local_jobs {
            let retiring_key = retiring_key.clone();
            let retiring_comms = retiring_comms.clone();
            let retiring_spec = retiring_spec.clone();
            let retired_id = ctx.agent_identity.clone();
            let retired_entry = ctx.entry.clone();
            local_tasks.push(async move {
                if let Some(retiring_comms) = retiring_comms.as_ref() {
                    if let Err(error) = actor
                        .notify_peer_retired(
                            &recipient_spec,
                            &retired_id,
                            &retired_entry,
                            &retiring_spec,
                            retiring_comms,
                        )
                        .await
                    {
                        if Self::is_peer_destroying_admission_rejection(&error) {
                            tracing::debug!(
                                mob_id = %actor.definition.id,
                                agent_identity = %retired_id,
                                peer_id = %peer_identity,
                                "dispose_notify_peers: peer rejected lifecycle notice (already retiring)"
                            );
                        } else {
                            return Err(error);
                        }
                    } else {
                        tracing::debug!(
                            mob_id = %actor.definition.id,
                            agent_identity = %retired_id,
                            peer_id = %peer_identity,
                            "dispose_notify_peers: lifecycle notice sent"
                        );
                    }
                } else {
                    tracing::debug!(
                        mob_id = %actor.definition.id,
                        agent_identity = %retired_id,
                        peer_id = %peer_identity,
                        "dispose_notify_peers: skipping lifecycle notice because retiring member has no live comms runtime"
                    );
                }
                actor
                    .apply_trusted_peer_remove(recipient_comms.as_ref(), retiring_key, authority)
                    .await
                    .map_err(MobError::from)?;
                Ok(())
            });
        }
        while let Some(result) = local_tasks.next().await {
            result?;
        }
        drop(local_tasks);
        for (peer_identity, recipient_spec, recipient_binding) in peer_only_jobs {
            if let Some(retiring_comms) = ctx.retiring_comms.as_ref() {
                if let Err(error) = self
                    .notify_peer_retired(
                        &recipient_spec,
                        &ctx.agent_identity,
                        &ctx.entry,
                        retiring_spec,
                        retiring_comms,
                    )
                    .await
                {
                    if Self::is_peer_destroying_admission_rejection(&error) {
                        tracing::debug!(
                            mob_id = %self.definition.id,
                            agent_identity = %ctx.agent_identity,
                            peer_id = %peer_identity,
                            "dispose_notify_peers: peer rejected lifecycle notice (already retiring)"
                        );
                    } else {
                        return Err(error);
                    }
                } else {
                    tracing::debug!(
                        mob_id = %self.definition.id,
                        agent_identity = %ctx.agent_identity,
                        peer_id = %peer_identity,
                        "dispose_notify_peers: lifecycle notice sent"
                    );
                }
            } else {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    agent_identity = %ctx.agent_identity,
                    peer_id = %peer_identity,
                    "dispose_notify_peers: skipping lifecycle notice because retiring member has no live comms runtime"
                );
            }
            self.cleanup_member_machine_wiring_edge(
                &ctx.entry.agent_identity,
                &peer_identity,
                "dispose_notify_peers_peer_only_machine_wiring_cleanup",
            )?;
            self.unwire_peer_only_recipient(
                &recipient_spec,
                Some(&recipient_binding),
                retiring_spec,
                std::time::Duration::from_secs(10),
            )
            .await?;
        }
        Ok(())
    }

    fn is_peer_destroying_admission_rejection(error: &MobError) -> bool {
        matches!(
            error,
            MobError::CommsError(meerkat_core::comms::SendError::AdmissionDropped {
                reason: meerkat_core::comms::AdmissionDropReason::ClassificationRejected
                    | meerkat_core::comms::AdmissionDropReason::SessionClosed,
            })
        )
    }

    /// Archive the member's session. Treats NotFound as success.
    pub(super) async fn dispose_archive_session(
        &self,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        tracing::info!(
            mob_id = %self.definition.id,
            agent_identity = %ctx.agent_identity,
            member_ref = ?ctx.entry.member_ref,
            "MobActor::dispose_archive_session retiring member via provisioner"
        );
        if let Err(error) = self.provisioner.retire_member(&ctx.entry.member_ref).await {
            if matches!(
                error,
                MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
            ) {
                return Ok(());
            }
            return Err(error);
        }
        tracing::info!(
            mob_id = %self.definition.id,
            agent_identity = %ctx.agent_identity,
            "MobActor::dispose_archive_session retired member via provisioner"
        );
        Ok(())
    }

    fn observe_member_retirement_archived(
        &mut self,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        self.apply_dsl_signal(
            mob_dsl::MobMachineSignal::ObserveMemberRetirementArchived {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&ctx.agent_identity),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&ctx.entry.agent_runtime_id),
                fence_token: mob_dsl::FenceToken::from_domain(ctx.entry.fence_token),
            },
            "dispose_member_archive_completed",
        )
    }

    /// Prune edge locks for the member. Infallible.
    async fn dispose_prune_edge_locks(&self, ctx: &DisposalContext) {
        self.edge_locks.prune(ctx.agent_identity.as_str()).await;
    }

    /// Remove the member from the roster. Infallible.
    pub(super) async fn dispose_remove_from_roster(&self, ctx: &DisposalContext) {
        let mut roster = self.roster.write().await;
        roster.remove_member(&ctx.agent_identity);
        drop(roster);
        self.restore_diagnostics
            .write()
            .await
            .remove(&ctx.agent_identity);
    }

    async fn handle_complete(&mut self) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_complete preflight")?;
        self.ensure_flow_tracker_alignment("handle_complete preflight")
            .await?;
        self.cancel_all_flow_tasks().await?;

        self.notify_orchestrator_lifecycle(format!("Mob '{}' is completing.", self.definition.id))
            .await;
        self.retire_all_members("complete").await?;

        // MobMachine owns both completion admission and the durable journal
        // request. The recovery event is appended only after the prepared
        // transition is accepted by the live generated authority.
        let prepared = self
            .prepare_dsl_input_transition(mob_dsl::MobMachineInput::Complete, "complete_input")
            .map_err(|error| {
                tracing::debug!(
                    context = "complete_input",
                    error = %error,
                    "MobMachine command admission rejected input"
                );
                self.invalid_transition_to(MobState::Completed)
            })?;
        Self::require_lifecycle_journal_effect(
            &prepared.transition,
            mob_dsl::MobLifecycleJournalKind::Completed,
            "complete_input",
        )?;
        let events = self.events.clone();
        let mob_id = self.definition.id.clone();
        self.commit_prepared_dsl_transition_after(prepared, move || async move {
            events
                .append(NewMobEvent {
                    mob_id,
                    timestamp: None,
                    kind: MobEventKind::MobCompleted,
                })
                .await
                .map_err(MobError::from)?;
            Ok(())
        })
        .await?;
        self.ensure_pending_spawn_alignment("handle_complete completion")?;
        self.ensure_flow_tracker_alignment("handle_complete completion")
            .await?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn remote_destroy_cleanup_deadline(remote_member_count: usize) -> std::time::Duration {
        let batches = remote_member_count
            .saturating_add(MAX_PARALLEL_REMOTE_MEMBER_TEARDOWNS.saturating_sub(1))
            / MAX_PARALLEL_REMOTE_MEMBER_TEARDOWNS.max(1);
        let deadline_secs = std::cmp::min(90, 5 + (batches as u64) * 17);
        std::time::Duration::from_secs(deadline_secs)
    }

    fn push_unique_identity(target: &mut Vec<AgentIdentity>, identity: AgentIdentity) {
        if !target.iter().any(|existing| existing == &identity) {
            target.push(identity);
        }
    }

    fn runtime_binding_for_entry(entry: &RosterEntry) -> Option<crate::RuntimeBinding> {
        Self::runtime_binding_for_member_ref(&entry.member_ref)
    }

    fn runtime_binding_for_member_ref(member_ref: &MemberRef) -> Option<crate::RuntimeBinding> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                session_id: None,
            } => Some(crate::RuntimeBinding::External {
                peer_id: peer_id.clone(),
                address: super::bridge_protocol::canonicalize_bridge_address(address),
                bootstrap_token: bootstrap_token.clone(),
                pubkey: *pubkey,
            }),
            _ => None,
        }
    }

    fn sanitized_member_ref(member_ref: &MemberRef) -> MemberRef {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                session_id,
                ..
            } => MemberRef::BackendPeer {
                peer_id: peer_id.clone(),
                address: super::bridge_protocol::canonicalize_bridge_address(address),
                pubkey: *pubkey,
                bootstrap_token: bootstrap_token.clone(),
                session_id: session_id.clone(),
            },
            MemberRef::Session { session_id } => MemberRef::Session {
                session_id: session_id.clone(),
            },
        }
    }

    fn external_binding_overlay_record(
        &self,
        agent_identity: &AgentIdentity,
        generation: crate::ids::Generation,
        member_ref: &MemberRef,
    ) -> Option<crate::store::ExternalBindingOverlayRecord> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                session_id: None,
            } => Some(crate::store::ExternalBindingOverlayRecord {
                agent_identity: agent_identity.clone(),
                generation,
                normalized_member_ref: Some(MemberRef::BackendPeer {
                    peer_id: peer_id.clone(),
                    address: super::bridge_protocol::canonicalize_bridge_address(address),
                    pubkey: *pubkey,
                    bootstrap_token: None,
                    session_id: None,
                }),
                bootstrap_token: bootstrap_token.clone(),
                status: crate::store::ExternalBindingOverlayStatus::Normalized,
                updated_at: chrono::Utc::now(),
            }),
            _ => None,
        }
    }

    async fn delete_external_binding_overlay_for_member(
        &self,
        agent_identity: &AgentIdentity,
        generation: crate::ids::Generation,
    ) -> Result<(), MobError> {
        self.runtime_metadata
            .delete_external_binding_overlay(&self.definition.id, agent_identity, generation)
            .await
            .map_err(MobError::from)
    }

    async fn record_destroy_member_retirement_archived(
        &mut self,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        let session_id_for_journal = self
            .dsl_authority
            .state()
            .member_session_bindings
            .get(&mob_dsl::AgentIdentity::from_domain(
                &ctx.entry.agent_identity,
            ))
            .cloned();
        let prepared = self.prepare_dsl_signal_transition(
            mob_dsl::MobMachineSignal::ObserveDestroyMemberRetirementArchived {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&ctx.entry.agent_identity),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&ctx.entry.agent_runtime_id),
                fence_token: mob_dsl::FenceToken::from_domain(ctx.entry.fence_token),
                generation: mob_dsl::Generation::from_domain(ctx.entry.generation),
                session_id: session_id_for_journal.clone(),
            },
            "destroy_member_archive_completed",
        )?;
        Self::require_member_lifecycle_journal_effect(
            &prepared.transition,
            mob_dsl::MobLifecycleJournalKind::MemberRetired,
            &ctx.entry.agent_identity,
            &ctx.entry.agent_runtime_id,
            None,
            ctx.entry.generation,
            session_id_for_journal,
            "destroy_member_archive_completed",
        )?;
        if !self
            .retire_event_exists(&ctx.entry.agent_identity, &ctx.entry.member_ref)
            .await?
        {
            self.append_retire_event_for_entry(&ctx.entry).await?;
        }
        self.commit_prepared_dsl_transition(prepared)?;
        Ok(())
    }

    async fn record_destroy_member_retired_event(
        &self,
        entry: &RosterEntry,
    ) -> Result<(), MobError> {
        let retire_event_already_present = self
            .retire_event_exists(&entry.agent_identity, &entry.member_ref)
            .await?;
        if !retire_event_already_present {
            return Err(MobError::Internal(format!(
                "destroy cleanup for '{}' reached disposal without a generated durable retire journal event",
                entry.agent_identity
            )));
        }
        Ok(())
    }

    async fn record_destroying_event(&mut self) -> Result<(), MobError> {
        let destroying_event_exists = self.destroying_event_exists().await?;
        if !destroying_event_exists {
            if self.destroy_admitted() {
                return Err(MobError::Internal(
                    "destroy cleanup was admitted without a durable MobDestroying journal event"
                        .to_string(),
                ));
            }
            let prepared = self.prepare_dsl_signal_transition(
                mob_dsl::MobMachineSignal::AdmitDestroyCleanup,
                "record_destroying_event",
            )?;
            Self::require_lifecycle_journal_effect(
                &prepared.transition,
                mob_dsl::MobLifecycleJournalKind::Destroying,
                "record_destroying_event",
            )?;
            let events = self.events.clone();
            let mob_id = self.definition.id.clone();
            self.commit_prepared_dsl_transition_after(prepared, move || async move {
                events
                    .append(NewMobEvent {
                        mob_id,
                        timestamp: None,
                        kind: MobEventKind::MobDestroying,
                    })
                    .await
                    .map_err(MobError::from)?;
                Ok(())
            })
            .await?;
        } else if !self.destroy_admitted() {
            let prepared = self.prepare_dsl_signal_transition(
                mob_dsl::MobMachineSignal::AdmitDestroyCleanup,
                "record_destroying_event",
            )?;
            Self::require_lifecycle_journal_effect(
                &prepared.transition,
                mob_dsl::MobLifecycleJournalKind::Destroying,
                "record_destroying_event",
            )?;
            self.commit_prepared_dsl_transition(prepared)?;
        }
        let _ = self.phase_watch_tx.send(self.state());
        Ok(())
    }

    async fn destroying_event_exists(&self) -> Result<bool, MobError> {
        let events = self.events.replay_all().await?;
        let epoch_start = events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |pos| pos + 1);
        Ok(events[epoch_start..]
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobDestroying)))
    }

    async fn destroy_storage_finalizing_event_exists(&self) -> Result<bool, MobError> {
        let events = self.events.replay_all().await?;
        let epoch_start = events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |pos| pos + 1);
        Ok(events[epoch_start..]
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobDestroyStorageFinalizing)))
    }

    async fn record_destroy_storage_finalizing_event(&mut self) -> Result<(), MobError> {
        if self.destroy_storage_finalizing_event_exists().await? {
            return Ok(());
        }
        let prepared = self.prepare_dsl_signal_transition(
            mob_dsl::MobMachineSignal::AdmitDestroyStorageFinalizing,
            "record_destroy_storage_finalizing_event",
        )?;
        Self::require_lifecycle_journal_effect(
            &prepared.transition,
            mob_dsl::MobLifecycleJournalKind::DestroyStorageFinalizing,
            "record_destroy_storage_finalizing_event",
        )?;
        let events = self.events.clone();
        let mob_id = self.definition.id.clone();
        self.commit_prepared_dsl_transition_after(prepared, move || async move {
            events
                .append(NewMobEvent {
                    mob_id,
                    timestamp: None,
                    kind: MobEventKind::MobDestroyStorageFinalizing,
                })
                .await
                .map_err(MobError::from)?;
            Ok(())
        })
        .await?;
        Ok(())
    }

    async fn runtime_metadata_snapshot(&self) -> Result<RuntimeMetadataSnapshot, MobError> {
        Ok(RuntimeMetadataSnapshot {
            supervisor: self
                .runtime_metadata
                .load_supervisor_authority(&self.definition.id)
                .await?,
            external_binding_overlays: self
                .runtime_metadata
                .list_external_binding_overlays(&self.definition.id)
                .await?,
        })
    }

    async fn restore_runtime_metadata_snapshot(
        &mut self,
        snapshot: &RuntimeMetadataSnapshot,
    ) -> Result<(), MobError> {
        if let Some(supervisor) = &snapshot.supervisor {
            match self
                .runtime_metadata
                .load_supervisor_authority(&self.definition.id)
                .await?
            {
                Some(current) if current == *supervisor => {}
                Some(current) => {
                    return Err(MobError::from(crate::store::MobStoreError::CasConflict(
                        format!(
                            "supervisor authority changed while restoring metadata for mob '{}': current peer={} epoch={}, snapshot peer={} epoch={}",
                            self.definition.id,
                            current.public_peer_id,
                            current.epoch,
                            supervisor.public_peer_id,
                            supervisor.epoch,
                        ),
                    )));
                }
                None => {
                    let prepared = self.prepare_supervisor_authority_persistence(
                        supervisor.dsl_restore_after_destroy_rollback_input(),
                        supervisor,
                        "restore_runtime_metadata_snapshot",
                    )?;
                    let inserted = self
                        .runtime_metadata
                        .put_supervisor_authority_if_absent(
                            &self.definition.id,
                            supervisor,
                            &prepared.authority,
                        )
                        .await?;
                    if !inserted {
                        return Err(MobError::from(crate::store::MobStoreError::CasConflict(
                            format!(
                                "supervisor authority changed while restoring absent metadata for mob '{}'",
                                self.definition.id
                            ),
                        )));
                    }
                    self.commit_prepared_dsl_transition(prepared.transition)?;
                }
            }
        }
        for overlay in &snapshot.external_binding_overlays {
            self.runtime_metadata
                .upsert_external_binding_overlay(&self.definition.id, overlay)
                .await?;
        }
        Ok(())
    }

    async fn incomplete_after_metadata_scrub_error(
        &mut self,
        mut report: super::handle::MobDestroyReport,
        snapshot: &RuntimeMetadataSnapshot,
        error: impl std::fmt::Display,
    ) -> super::handle::MobDestroyError {
        report.push_error(error.to_string());
        if let Err(restore_error) = self.restore_runtime_metadata_snapshot(snapshot).await {
            report.push_error(format!(
                "runtime metadata restore failed after incomplete destroy: {restore_error}"
            ));
        }
        report.metadata_scrubbed = false;
        super::handle::MobDestroyError::Incomplete { report }
    }

    fn incomplete_destroy_error(
        mut report: super::handle::MobDestroyReport,
        context: &str,
        error: impl std::fmt::Display,
    ) -> super::handle::MobDestroyError {
        report.push_error(format!("{context}: {error}"));
        super::handle::MobDestroyError::Incomplete { report }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn expected_revoke_cleanup_failure(error: &MobError) -> Option<ExpectedRevokeCleanupFailure> {
        match error {
            MobError::CommsError(meerkat_core::comms::SendError::PeerNotFound(_)) => {
                Some(ExpectedRevokeCleanupFailure::CommsPeerNotFound)
            }
            MobError::CommsError(meerkat_core::comms::SendError::PeerOffline) => {
                Some(ExpectedRevokeCleanupFailure::CommsPeerOffline)
            }
            MobError::CommsError(meerkat_core::comms::SendError::AdmissionDropped { reason }) => {
                match reason {
                    meerkat_core::comms::AdmissionDropReason::UntrustedSender
                    | meerkat_core::comms::AdmissionDropReason::SessionClosed => {
                        Some(ExpectedRevokeCleanupFailure::CommsAdmissionDropped {
                            reason: *reason,
                        })
                    }
                    _ => None,
                }
            }
            MobError::BridgeCommandRejected {
                cause: super::bridge_protocol::BridgeRejectionCause::NotBound,
                ..
            } => Some(ExpectedRevokeCleanupFailure::BridgeRejected {
                cause: super::bridge_protocol::BridgeRejectionCause::NotBound,
            }),
            _ => None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn destroy_remote_member_for_destroy(
        &mut self,
        entry: RosterEntry,
        trust_cleanup_plan: RetireTrustCleanupPlan,
    ) -> RemoteDestroyOutcome {
        let identity = entry.agent_identity.clone();
        let agent_identity = entry.agent_identity.clone();
        let mut outcome = RemoteDestroyOutcome {
            identity: identity.clone(),
            force_destroyed: false,
            orphaned: false,
            errors: Vec::new(),
        };
        let Some(binding) = Self::runtime_binding_for_entry(&entry) else {
            outcome
                .errors
                .push("remote destroy requested for non peer-only member".to_string());
            outcome.orphaned = true;
            return outcome;
        };

        let ctx = self
            .disposal_context_from_entry(&agent_identity, &entry, trust_cleanup_plan)
            .await;
        let disposal = self.dispose_member_for_destroy(&ctx).await;

        let disposal_error = Self::destroy_disposal_failure(&disposal);
        let mut remote_cleanup_complete = disposal_error.is_none();
        if let Some(error) = disposal_error {
            outcome
                .errors
                .push(format!("graceful retire failed: {error}"));

            match self
                .observe_peer_only_binding(&binding, std::time::Duration::from_millis(750))
                .await
            {
                Ok(observation) => match self.observation_is_terminal(&observation) {
                    // Mirror MobMachine's terminality verdict. A non-terminal
                    // verdict (or a fail-closed classification error) leaves
                    // `remote_cleanup_complete` false so the force-destroy path
                    // below runs.
                    Ok(true) => {
                        remote_cleanup_complete = true;
                    }
                    Ok(false) => {
                        outcome.errors.push(format!(
                            "confirmatory observation reported non-terminal state {}",
                            observation.state
                        ));
                    }
                    Err(error) => outcome.errors.push(format!(
                        "confirmatory observation terminality classification failed: {error}"
                    )),
                },
                Err(error) => outcome
                    .errors
                    .push(format!("confirmatory observation failed: {error}")),
            }

            if !remote_cleanup_complete {
                match self
                    .destroy_peer_only_binding(&binding, std::time::Duration::from_secs(5))
                    .await
                {
                    Ok(_) => {
                        remote_cleanup_complete = true;
                        outcome.force_destroyed = true;
                    }
                    Err(error) => outcome
                        .errors
                        .push(format!("force destroy failed: {error}")),
                }
            }
        }

        let mut revoke_failed = false;
        if let Err(error) = self
            .revoke_supervisor_for_binding(&binding, std::time::Duration::from_secs(5))
            .await
        {
            let expected_after_destroy = remote_cleanup_complete
                .then(|| Self::expected_revoke_cleanup_failure(&error))
                .flatten();
            if let Some(cause) = expected_after_destroy {
                let peer_id = match &binding {
                    crate::RuntimeBinding::External { peer_id, .. } => peer_id.as_str(),
                    crate::RuntimeBinding::Session => "session",
                };
                tracing::debug!(
                    peer_id = %peer_id,
                    ?cause,
                    error = %error,
                    "destroy cleanup: supervisor revoke failed after terminal remote cleanup"
                );
            } else {
                revoke_failed = true;
                outcome
                    .errors
                    .push(format!("supervisor revoke failed: {error}"));
            }
        }

        outcome.orphaned = !remote_cleanup_complete || revoke_failed;
        if !outcome.orphaned {
            if let Err(error) = self.record_destroy_member_retired_event(&entry).await {
                outcome
                    .errors
                    .push(format!("durable retire event append failed: {error}"));
                outcome.orphaned = true;
                return outcome;
            }
            self.dispose_prune_edge_locks(&ctx).await;
            self.dispose_remove_from_roster(&ctx).await;
        }
        outcome
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn destroy_remote_members_for_destroy(
        &mut self,
        remote_entries: Vec<RosterEntry>,
        trust_unwire_authority_by_member: &mut BTreeMap<MeerkatId, RetireTrustCleanupPlan>,
        report: &mut super::handle::MobDestroyReport,
    ) {
        if remote_entries.is_empty() {
            return;
        }

        // Phase 2 sequential expedient: dispose_member currently takes
        // `&mut self` (via `dispose_stop_host_loop` / `stop_autonomous_member`).
        // Phase 4 will re-introduce FuturesUnordered parallelism once the
        // disposal steps are converted to `&self` so a shared actor reference
        // can be held across concurrent disposal futures.
        let deadline = Self::remote_destroy_cleanup_deadline(remote_entries.len());
        let deadline_at = tokio::time::Instant::now() + deadline;
        let mut remaining = VecDeque::from(remote_entries);

        while let Some(entry) = remaining.pop_front() {
            let identity = entry.agent_identity.clone();
            let trust_cleanup_plan = trust_unwire_authority_by_member
                .remove(&identity)
                .unwrap_or_else(RetireTrustCleanupPlan::empty);
            let next = tokio::time::timeout_at(
                deadline_at,
                self.destroy_remote_member_for_destroy(entry, trust_cleanup_plan),
            )
            .await;
            let outcome = match next {
                Ok(outcome) => outcome,
                Err(_) => {
                    report.remote_cleanup_deadline_exceeded = true;
                    Self::push_unique_identity(&mut report.orphaned_remote_members, identity);
                    for entry in remaining {
                        Self::push_unique_identity(
                            &mut report.orphaned_remote_members,
                            entry.agent_identity.clone(),
                        );
                    }
                    return;
                }
            };

            if outcome.force_destroyed {
                Self::push_unique_identity(
                    &mut report.force_destroyed_members,
                    outcome.identity.clone(),
                );
            }
            if outcome.orphaned {
                Self::push_unique_identity(
                    &mut report.orphaned_remote_members,
                    outcome.identity.clone(),
                );
            }
            for error in outcome.errors {
                report.push_error(format!("{}: {error}", outcome.identity));
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn destroy_remote_members_for_destroy(
        &self,
        remote_entries: Vec<RosterEntry>,
        _trust_unwire_authority_by_member: &mut BTreeMap<MeerkatId, RetireTrustCleanupPlan>,
        report: &mut super::handle::MobDestroyReport,
    ) {
        for entry in remote_entries {
            Self::push_unique_identity(
                &mut report.orphaned_remote_members,
                entry.agent_identity.clone(),
            );
        }
    }

    async fn dispose_local_member_after_destroy_admission(
        &mut self,
        entry: RosterEntry,
        trust_cleanup_plan: RetireTrustCleanupPlan,
        report: &mut super::handle::MobDestroyReport,
    ) -> Result<(), super::handle::MobDestroyError> {
        let ctx = self
            .disposal_context_from_entry(&entry.agent_identity, &entry, trust_cleanup_plan)
            .await;
        let disposal_report = self.dispose_member_for_destroy(&ctx).await;
        if let Some(error) = Self::destroy_disposal_failure(&disposal_report) {
            report.push_error(format!("{}: {error}", entry.agent_identity));
            return Err(super::handle::MobDestroyError::Incomplete {
                report: report.clone(),
            });
        }
        if let Err(error) = self.record_destroy_member_retired_event(&entry).await {
            report.push_error(format!(
                "{}: durable retire event append failed: {error}",
                entry.agent_identity
            ));
            return Err(super::handle::MobDestroyError::Incomplete {
                report: report.clone(),
            });
        }
        self.dispose_prune_edge_locks(&ctx).await;
        self.dispose_remove_from_roster(&ctx).await;
        if let Err(error) = self
            .delete_external_binding_overlay_for_member(&entry.agent_identity, entry.generation)
            .await
        {
            report.push_error(error.to_string());
            return Err(super::handle::MobDestroyError::Incomplete {
                report: report.clone(),
            });
        }
        Ok(())
    }

    async fn handle_destroy(
        &mut self,
    ) -> Result<super::handle::MobDestroyReport, super::handle::MobDestroyError> {
        let was_active = self.destroy_cleanup_active;
        self.destroy_cleanup_active = true;
        let result = self.handle_destroy_inner().await;
        self.destroy_cleanup_active = was_active;
        result
    }

    async fn handle_destroy_inner(
        &mut self,
    ) -> Result<super::handle::MobDestroyReport, super::handle::MobDestroyError> {
        use super::handle::{MobDestroyError, MobDestroyReport};

        let mut report = MobDestroyReport::default();
        let destroy_input_needed = self.dsl_state() != crate::runtime::MobState::Destroyed;

        self.ensure_pending_spawn_alignment("handle_destroy preflight")
            .map_err(|error| {
                if self.destroy_admitted() {
                    Self::incomplete_destroy_error(
                        report.clone(),
                        "pending spawn alignment during admitted destroy failed",
                        error,
                    )
                } else {
                    MobDestroyError::from(error)
                }
            })?;
        self.ensure_flow_tracker_alignment("handle_destroy preflight")
            .await
            .map_err(|error| {
                if self.destroy_admitted() {
                    Self::incomplete_destroy_error(
                        report.clone(),
                        "flow tracker alignment during admitted destroy failed",
                        error,
                    )
                } else {
                    MobDestroyError::from(error)
                }
            })?;
        let entries = {
            let roster = self.roster.read().await;
            roster.list_all().cloned().collect::<Vec<_>>()
        };
        let mut trust_cleanup_plan_by_member: BTreeMap<MeerkatId, RetireTrustCleanupPlan> =
            BTreeMap::new();
        if destroy_input_needed {
            for entry in &entries {
                let plan = match self
                    .member_retire_trust_cleanup_plan(&entry.agent_identity, entry)
                    .await
                {
                    Ok(plan) => plan,
                    Err(error) => {
                        report.push_error(format!(
                            "{}: destroy retire trust authority failed: {error}",
                            entry.agent_identity
                        ));
                        return Err(MobDestroyError::Incomplete { report });
                    }
                };
                if plan.has_peers() {
                    trust_cleanup_plan_by_member.insert(entry.agent_identity.clone(), plan);
                }
            }
        }
        if destroy_input_needed && let Err(error) = self.record_destroying_event().await {
            report.push_error(format!("destroy marker append failed: {error}"));
            return Err(MobDestroyError::Incomplete { report });
        }
        self.fail_all_pending_spawns("mob is destroying")
            .await
            .map_err(|error| {
                Self::incomplete_destroy_error(
                    report.clone(),
                    "pending spawn cleanup during destroy failed",
                    error,
                )
            })?;
        self.cancel_pending_peer_deliveries("mob is destroying")
            .await;
        if destroy_input_needed && self.has_orchestrator {
            self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::StopOrchestrator,
                "stop_orchestrator_destroy",
            )
            .map_err(|error| {
                Self::incomplete_destroy_error(
                    report.clone(),
                    "stop orchestrator during destroy failed",
                    error,
                )
            })?;
            self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::DestroyOrchestrator,
                "destroy_orchestrator",
            )
            .map_err(|error| {
                Self::incomplete_destroy_error(
                    report.clone(),
                    "destroy orchestrator during destroy failed",
                    error,
                )
            })?;
        }
        if destroy_input_needed {
            for entry in &entries {
                if let Err(error) = self.admit_member_retire_for_destroy(entry).await {
                    report.push_error(format!(
                        "{}: destroy retire admission failed: {error}",
                        entry.agent_identity
                    ));
                    return Err(MobDestroyError::Incomplete { report });
                }
            }
        }
        self.cancel_all_flow_tasks().await.map_err(|error| {
            Self::incomplete_destroy_error(
                report.clone(),
                "cancel flow tasks during destroy failed",
                error,
            )
        })?;
        if !self.pending_routed_effects.is_empty() {
            self.flush_routed_effects().await.map_err(|error| {
                Self::incomplete_destroy_error(
                    report.clone(),
                    "destroy routed effect dispatch failed",
                    error,
                )
            })?;
        }
        if destroy_input_needed {
            self.notify_orchestrator_lifecycle(format!(
                "Mob '{}' is destroying.",
                self.definition.id
            ))
            .await;
        }
        let (remote_entries, local_entries): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .partition(|entry| Self::runtime_binding_for_entry(entry).is_some());

        for entry in local_entries {
            let plan = trust_cleanup_plan_by_member
                .remove(&entry.agent_identity)
                .unwrap_or_else(RetireTrustCleanupPlan::empty);
            self.dispose_local_member_after_destroy_admission(entry, plan, &mut report)
                .await?;
        }
        self.destroy_remote_members_for_destroy(
            remote_entries,
            &mut trust_cleanup_plan_by_member,
            &mut report,
        )
        .await;
        if report.remote_cleanup_deadline_exceeded
            || !report.orphaned_remote_members.is_empty()
            || !report.errors.is_empty()
        {
            return Err(MobDestroyError::Incomplete { report });
        }
        if destroy_input_needed {
            self.apply_dsl_input(mob_dsl::MobMachineInput::Destroy, "destroy_input")
                .map_err(|error| {
                    Self::incomplete_destroy_error(
                        report.clone(),
                        "destroy machine transition failed",
                        error,
                    )
                })?;
        }
        self.ensure_pending_spawn_alignment("handle_destroy completion")
            .map_err(|error| {
                let mut report = report.clone();
                report.push_error(error.to_string());
                MobDestroyError::Incomplete { report }
            })?;
        self.ensure_flow_tracker_alignment("handle_destroy completion")
            .await
            .map_err(|error| {
                let mut report = report.clone();
                report.push_error(error.to_string());
                MobDestroyError::Incomplete { report }
            })?;
        if let Err(error) = self.cleanup_namespace().await {
            report.push_error(error.to_string());
            return Err(MobDestroyError::Incomplete { report });
        }
        report.namespace_cleaned = true;
        self.record_destroy_storage_finalizing_event()
            .await
            .map_err(|error| {
                Self::incomplete_destroy_error(
                    report.clone(),
                    "record destroy storage finalizing event failed",
                    error,
                )
            })?;
        let runtime_metadata_snapshot =
            self.runtime_metadata_snapshot().await.map_err(|error| {
                let mut report = report.clone();
                report.push_error(error.to_string());
                MobDestroyError::Incomplete { report }
            })?;
        if let Err(error) = self
            .runtime_metadata
            .delete_external_binding_overlays(&self.definition.id)
            .await
        {
            return Err(self
                .incomplete_after_metadata_scrub_error(report, &runtime_metadata_snapshot, error)
                .await);
        }
        let supervisor_delete = match runtime_metadata_snapshot.supervisor.as_ref() {
            Some(supervisor) => Some(
                self.prepare_supervisor_authority_deletion(
                    supervisor,
                    "handle_destroy metadata scrub",
                )
                .map_err(|error| {
                    Self::incomplete_destroy_error(
                        report.clone(),
                        "supervisor authority deletion admission failed",
                        error,
                    )
                })?,
            ),
            None => None,
        };
        if let (Some(supervisor), Some(prepared)) = (
            runtime_metadata_snapshot.supervisor.as_ref(),
            supervisor_delete,
        ) {
            if let Err(error) = self
                .runtime_metadata
                .delete_supervisor_authority(&self.definition.id, supervisor, &prepared.authority)
                .await
                .and_then(|deleted| {
                    if deleted {
                        Ok(())
                    } else {
                        Err(crate::store::MobStoreError::CasConflict(format!(
                            "supervisor authority changed while deleting metadata for mob '{}'",
                            self.definition.id
                        )))
                    }
                })
            {
                return Err(self
                    .incomplete_after_metadata_scrub_error(
                        report,
                        &runtime_metadata_snapshot,
                        error,
                    )
                    .await);
            }
            self.commit_prepared_dsl_transition(prepared.transition)?;
        }
        report.metadata_scrubbed = true;
        if let Err(error) = self.events.clear().await {
            return Err(self
                .incomplete_after_metadata_scrub_error(report, &runtime_metadata_snapshot, error)
                .await);
        }
        report.events_cleared = true;
        self.edge_locks.clear().await;
        if report.remote_cleanup_deadline_exceeded
            || !report.orphaned_remote_members.is_empty()
            || !report.errors.is_empty()
        {
            return Err(MobDestroyError::Incomplete { report });
        }
        Ok(report)
    }

    async fn handle_rotate_supervisor(
        &mut self,
    ) -> Result<super::handle::SupervisorRotationReport, MobError> {
        if self.state() == MobState::Destroyed {
            return Err(self.invalid_transition_to(MobState::Destroyed));
        }
        let loaded = self
            .load_supervisor_authority_snapshot()
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "cannot rotate supervisor for mob '{}': missing supervisor runtime metadata",
                    self.definition.id
                ))
            })?;
        let current = loaded.durable;
        let mut durable_write_expected = current.clone();
        let stable_current = current.without_pending_rotation();
        let mut pending_rotation = current.pending_rotation.clone();
        let mut accepted_peer_ids: BTreeSet<String> = pending_rotation
            .as_ref()
            .map(|pending| pending.accepted_peer_ids.iter().cloned().collect())
            .unwrap_or_default();
        let mut next = pending_rotation
            .as_ref()
            .map(|pending| pending.authority_record())
            .unwrap_or_else(|| {
                let mut generated =
                    crate::store::SupervisorAuthorityRecord::generate(current.protocol_version);
                generated.epoch = current.epoch + 1;
                generated
            });
        next.pending_rotation = None;
        let remote_bindings = {
            let roster = self.roster.read().await;
            roster
                .list_all()
                .filter_map(Self::runtime_binding_for_entry)
                .collect::<Vec<_>>()
        };
        let mut rotated_peers: Vec<(TrustedPeerDescriptor, crate::RuntimeBinding)> = Vec::new();
        self.rotate_supervisor_bridge_to(&stable_current).await?;
        if let Some(pending) = pending_rotation.clone()
            && !pending.accepted_peer_ids.is_empty()
            && pending.epoch <= stable_current.epoch
            && pending.public_peer_id != stable_current.public_peer_id
        {
            let repair_accepted_peer_ids: BTreeSet<String> =
                pending.accepted_peer_ids.iter().cloned().collect();
            let attempted = pending.authority_record();
            let peers_to_reconcile = self.rotated_peer_bindings_for_accepted_peer_ids(
                &remote_bindings,
                &repair_accepted_peer_ids,
                &[],
            )?;
            let repair = self
                .reconcile_attempted_supervisor_peers_to_authority(
                    &attempted,
                    &stable_current,
                    &peers_to_reconcile,
                )
                .await;
            if let Err(error) = repair {
                return Err(MobError::SupervisorRotationIncomplete {
                    previous_epoch: stable_current.epoch,
                    attempted_epoch: attempted.epoch,
                    attempted_public_peer_id: attempted.public_peer_id,
                    rotated_peer_count: repair_accepted_peer_ids.len(),
                    rollback_succeeded: false,
                    pending_authority_recorded: true,
                    pending_authority_process_local: false,
                    rollback_error: Some(error.to_string()),
                    reason: "failed to reconcile stale accepted supervisor authority before retry"
                        .to_string(),
                });
            }
            let cleared = stable_current.without_pending_rotation();
            let prepared_clear = self.prepare_supervisor_authority_persistence(
                cleared.dsl_clear_pending_rotation_input(),
                &cleared,
                "clear_stale_supervisor_pending_repair",
            )?;
            match self
                .runtime_metadata
                .compare_and_put_supervisor_authority(
                    &self.definition.id,
                    &durable_write_expected,
                    &cleared,
                    &prepared_clear.authority,
                )
                .await
            {
                Ok(true) => {
                    self.commit_prepared_dsl_transition(prepared_clear.transition)?;
                    durable_write_expected = cleared;
                }
                Ok(false) => {
                    return Err(MobError::from(crate::store::MobStoreError::CasConflict(
                        format!(
                            "supervisor authority changed while clearing stale pending repair for mob '{}'",
                            self.definition.id
                        ),
                    )));
                }
                Err(error) => return Err(MobError::from(error)),
            }
            pending_rotation = None;
            accepted_peer_ids.clear();
            next = {
                let mut generated =
                    crate::store::SupervisorAuthorityRecord::generate(current.protocol_version);
                generated.epoch = stable_current.epoch + 1;
                generated
            };
        }
        if !remote_bindings.is_empty() {
            let next_public_key = next.keypair().public_key();
            let next_sup_spec: super::bridge_protocol::BridgePeerSpec =
                meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
                    format!("{}/__mob_supervisor__", self.definition.id),
                    next.public_peer_id.clone(),
                    *next_public_key.as_bytes(),
                    format!("inproc://{}/__mob_supervisor__", self.definition.id),
                )
                .map_err(|error| {
                    MobError::WiringError(format!("invalid rotated supervisor spec: {error}"))
                })?
                .into();
            let next_payload = super::bridge_protocol::BridgeSupervisorPayload {
                supervisor: next_sup_spec,
                epoch: next.epoch,
                protocol_version: next.protocol_version,
            };
            let current_payload = super::bridge_protocol::BridgeSupervisorPayload {
                supervisor: Self::supervisor_spec_for_authority(
                    &self.definition.id,
                    &stable_current,
                )?
                .into(),
                epoch: stable_current.epoch,
                protocol_version: stable_current.protocol_version,
            };
            let next_command =
                super::bridge_protocol::BridgeCommand::AuthorizeSupervisor(next_payload.clone());
            for binding in remote_bindings.iter().cloned() {
                let peer = Self::peer_only_spec_for_binding(&binding, "handle_rotate_supervisor")?;
                let peer_id = peer.peer_id.to_string();
                let mut already_pending = accepted_peer_ids.contains(&peer_id);
                if already_pending {
                    match self
                        .pending_supervisor_acceptance_confirmed(
                            &peer,
                            &next,
                            &next_command,
                            next_payload.protocol_version,
                        )
                        .await
                    {
                        Ok(true) => continue,
                        Ok(false) => {
                            accepted_peer_ids.remove(&peer_id);
                            already_pending = false;
                        }
                        Err(error) => {
                            self.rotate_supervisor_bridge_to(&stable_current).await?;
                            let reason = format!(
                                "failed to verify pending supervisor authority for peer '{peer_id}': {error}"
                            );
                            let pending_persistence = self
                                .persist_pending_supervisor_rotation(
                                    &stable_current,
                                    &durable_write_expected,
                                    &next,
                                    &accepted_peer_ids,
                                )
                                .await?;
                            let recovery = if !pending_persistence.pending_authority_recorded {
                                Some(
                                    self.recover_unrecorded_supervisor_attempt(
                                        &stable_current,
                                        &next,
                                        &accepted_peer_ids,
                                        &remote_bindings,
                                        &rotated_peers,
                                    )
                                    .await?,
                                )
                            } else {
                                None
                            };
                            return Err(MobError::SupervisorRotationIncomplete {
                                previous_epoch: stable_current.epoch,
                                attempted_epoch: next.epoch,
                                attempted_public_peer_id: next.public_peer_id.clone(),
                                rotated_peer_count: accepted_peer_ids.len(),
                                rollback_succeeded: recovery
                                    .as_ref()
                                    .is_some_and(|result| result.rollback_succeeded),
                                pending_authority_recorded: pending_persistence
                                    .pending_authority_recorded,
                                pending_authority_process_local: false,
                                rollback_error: recovery
                                    .as_ref()
                                    .and_then(|result| result.rollback_error.clone()),
                                reason,
                            });
                        }
                    }
                }
                let mut effective_peer = peer.clone();
                let mut effective_binding = binding.clone();
                self.supervisor_bridge.trust_recipient(&peer).await?;
                let authorize_result = self
                    .supervisor_bridge
                    .send_bridge_command(&peer, &next_command, std::time::Duration::from_secs(5))
                    .await;
                let authorize_error = match authorize_result {
                    Ok(value) => {
                        if let Some(rejection) =
                            Self::bridge_rejection_reply(next_payload.protocol_version, &value)
                        {
                            let should_rebind = match rejection.typed_cause() {
                                Some(cause) => self.classify_bridge_rejection_recovery(cause)?,
                                None => false,
                            };
                            if should_rebind {
                                let bind = self
                                    .bind_peer_only_member_for_binding_with_payload(
                                        &peer,
                                        &binding,
                                        &current_payload,
                                    )
                                    .await;
                                match bind {
                                    Ok(authorized_bind) => {
                                        let effective_bootstrap_token =
                                            match Self::bridge_bootstrap_token_from_binding(
                                                &binding,
                                            ) {
                                                Ok(token) => token,
                                                Err(error) => {
                                                    return Err(MobError::WiringError(format!(
                                                        "bind fallback restored current supervisor but failed to prepare rebound binding metadata: {error}"
                                                    )));
                                                }
                                            };
                                        let rebound_binding = crate::RuntimeBinding::External {
                                            peer_id: authorized_bind.peer.peer_id.to_string(),
                                            address:
                                                super::bridge_protocol::canonicalize_bridge_address(
                                                    &authorized_bind.peer.address.to_string(),
                                                ),
                                            bootstrap_token: Some(effective_bootstrap_token),
                                            pubkey: Some(authorized_bind.peer.pubkey),
                                        };
                                        effective_peer = authorized_bind.peer.clone();
                                        if let Err(error) = self
                                            .persist_rebound_binding(
                                                &binding,
                                                &authorized_bind.peer,
                                                &authorized_bind.response,
                                            )
                                            .await
                                        {
                                            return Err(MobError::WiringError(format!(
                                                "bind fallback restored current supervisor but failed to persist rebound binding metadata: {error}"
                                            )));
                                        }
                                        effective_binding = rebound_binding;
                                        self.supervisor_bridge
                                            .trust_recipient(&effective_peer)
                                            .await?;
                                        let retry = self
                                            .supervisor_bridge
                                            .send_bridge_command(
                                                &effective_peer,
                                                &next_command,
                                                std::time::Duration::from_secs(5),
                                            )
                                            .await;
                                        match retry {
                                            Ok(value) => {
                                                if let Some(retry_rejection) =
                                                    Self::bridge_rejection_reply(
                                                        next_payload.protocol_version,
                                                        &value,
                                                    )
                                                {
                                                    Some(Self::bridge_rejection_error_with_reason(
                                                        &retry_rejection,
                                                        format!(
                                                            "{}; bind fallback restored current supervisor but authorize retry failed: {}",
                                                            rejection.reason(),
                                                            retry_rejection.reason()
                                                        ),
                                                    ))
                                                } else if let Err(error) = serde_json::from_value::<
                                                    super::bridge_protocol::BridgeAck,
                                                >(
                                                    value
                                                ) {
                                                    Some(MobError::Internal(format!(
                                                        "failed to decode rotate supervisor response after bind fallback: {error}"
                                                    )))
                                                } else {
                                                    None
                                                }
                                            }
                                            Err(error) => Some(error),
                                        }
                                    }
                                    Err(bind_error) => {
                                        let reason = format!(
                                            "{}; bind fallback failed: {bind_error}",
                                            rejection.reason()
                                        );
                                        Some(Self::bridge_rejection_error_with_reason(
                                            &rejection, reason,
                                        ))
                                    }
                                }
                            } else {
                                Some(Self::bridge_rejection_error(rejection))
                            }
                        } else if let Err(error) =
                            serde_json::from_value::<super::bridge_protocol::BridgeAck>(value)
                        {
                            Some(MobError::Internal(format!(
                                "failed to decode rotate supervisor response: {error}"
                            )))
                        } else {
                            None
                        }
                    }
                    Err(error) => Some(error),
                };
                if let Some(error) = authorize_error {
                    let accepted_peer_count = accepted_peer_ids.len();
                    let reason = error.to_string();
                    if !rotated_peers.is_empty() {
                        match self
                            .rollback_rotated_supervisor_peers(&stable_current, &rotated_peers)
                            .await
                        {
                            Ok(()) => {
                                for (peer, _) in &rotated_peers {
                                    accepted_peer_ids.remove(&peer.peer_id.to_string());
                                }
                                self.rotate_supervisor_bridge_to(&stable_current).await?;
                                let pending_persistence = self
                                    .persist_pending_supervisor_rotation(
                                        &stable_current,
                                        &durable_write_expected,
                                        &next,
                                        &accepted_peer_ids,
                                    )
                                    .await?;
                                return Err(MobError::SupervisorRotationIncomplete {
                                    previous_epoch: stable_current.epoch,
                                    attempted_epoch: next.epoch,
                                    attempted_public_peer_id: next.public_peer_id.clone(),
                                    rotated_peer_count: accepted_peer_count,
                                    rollback_succeeded: true,
                                    pending_authority_recorded: pending_persistence
                                        .pending_authority_recorded,
                                    pending_authority_process_local: false,
                                    rollback_error: None,
                                    reason,
                                });
                            }
                            Err(rollback_error) => {
                                self.rotate_supervisor_bridge_to(&stable_current).await?;
                                let pending_persistence = self
                                    .persist_pending_supervisor_rotation(
                                        &stable_current,
                                        &durable_write_expected,
                                        &next,
                                        &accepted_peer_ids,
                                    )
                                    .await?;
                                return Err(MobError::SupervisorRotationIncomplete {
                                    previous_epoch: stable_current.epoch,
                                    attempted_epoch: next.epoch,
                                    attempted_public_peer_id: next.public_peer_id.clone(),
                                    rotated_peer_count: accepted_peer_count,
                                    rollback_succeeded: false,
                                    pending_authority_recorded: pending_persistence
                                        .pending_authority_recorded,
                                    pending_authority_process_local: false,
                                    rollback_error: Some(rollback_error.to_string()),
                                    reason,
                                });
                            }
                        }
                    }
                    if pending_rotation.is_some() {
                        self.rotate_supervisor_bridge_to(&stable_current).await?;
                        let pending_persistence = self
                            .persist_pending_supervisor_rotation(
                                &stable_current,
                                &durable_write_expected,
                                &next,
                                &accepted_peer_ids,
                            )
                            .await?;
                        return Err(MobError::SupervisorRotationIncomplete {
                            previous_epoch: stable_current.epoch,
                            attempted_epoch: next.epoch,
                            attempted_public_peer_id: next.public_peer_id.clone(),
                            rotated_peer_count: accepted_peer_count,
                            rollback_succeeded: false,
                            pending_authority_recorded: pending_persistence
                                .pending_authority_recorded,
                            pending_authority_process_local: false,
                            rollback_error: None,
                            reason,
                        });
                    }
                    self.rotate_supervisor_bridge_to(&stable_current).await?;
                    return Err(MobError::WiringError(format!(
                        "failed to rotate supervisor authority: {error}"
                    )));
                }
                if !already_pending {
                    accepted_peer_ids.insert(effective_peer.peer_id.to_string());
                    rotated_peers.push((effective_peer, effective_binding));
                    let pending_persistence = self
                        .persist_pending_supervisor_rotation(
                            &stable_current,
                            &durable_write_expected,
                            &next,
                            &accepted_peer_ids,
                        )
                        .await?;
                    if let Some(record) = pending_persistence.persisted_record.clone() {
                        durable_write_expected = record;
                    }
                    if !pending_persistence.pending_authority_recorded {
                        let recovery = self
                            .recover_unrecorded_supervisor_attempt(
                                &stable_current,
                                &next,
                                &accepted_peer_ids,
                                &remote_bindings,
                                &rotated_peers,
                            )
                            .await?;
                        return Err(MobError::SupervisorRotationIncomplete {
                            previous_epoch: stable_current.epoch,
                            attempted_epoch: next.epoch,
                            attempted_public_peer_id: next.public_peer_id.clone(),
                            rotated_peer_count: accepted_peer_ids.len(),
                            rollback_succeeded: recovery.rollback_succeeded,
	                            pending_authority_recorded: pending_persistence
	                                .pending_authority_recorded,
	                            pending_authority_process_local: false,
                            rollback_error: recovery.rollback_error,
                            reason:
                                "failed to persist pending supervisor rotation after a remote accepted attempted authority"
                                    .to_string(),
                        });
                    }
                }
            }
        }
        let public_peer_id = next.public_peer_id.clone();
        match self
            .activate_supervisor_authority(&stable_current, &durable_write_expected, &next)
            .await
        {
            Ok(()) => Ok(super::handle::SupervisorRotationReport {
                previous_epoch: stable_current.epoch,
                current_epoch: next.epoch,
                public_peer_id,
            }),
            Err(error) => {
                let has_remote_pending =
                    pending_rotation.is_some() || !accepted_peer_ids.is_empty();
                let mut rollback_succeeded = error.rollback_succeeded && !has_remote_pending;
                let mut rollback_error = error.rollback_error;
                let pending_persistence = if has_remote_pending {
                    self.persist_pending_supervisor_rotation(
                        &stable_current,
                        &durable_write_expected,
                        &next,
                        &accepted_peer_ids,
                    )
                    .await?
                } else {
                    SupervisorPendingRotationPersistence {
                        pending_authority_recorded: false,
                        persisted_record: None,
                    }
                };
                if has_remote_pending && !pending_persistence.pending_authority_recorded {
                    let recovery = self
                        .recover_unrecorded_supervisor_attempt(
                            &stable_current,
                            &next,
                            &accepted_peer_ids,
                            &remote_bindings,
                            &rotated_peers,
                        )
                        .await?;
                    rollback_succeeded = recovery.rollback_succeeded;
                    rollback_error = match (rollback_error, recovery.rollback_error) {
                        (Some(existing), Some(recovery_error)) => {
                            Some(format!("{existing}; {recovery_error}"))
                        }
                        (None, Some(recovery_error)) => Some(recovery_error),
                        (existing, None) => existing,
                    };
                }
                Err(MobError::SupervisorRotationIncomplete {
                    previous_epoch: stable_current.epoch,
                    attempted_epoch: next.epoch,
                    attempted_public_peer_id: next.public_peer_id.clone(),
                    rotated_peer_count: accepted_peer_ids.len(),
                    rollback_succeeded,
                    pending_authority_recorded: pending_persistence.pending_authority_recorded,
                    pending_authority_process_local: false,
                    rollback_error,
                    reason: format!(
                        "failed to commit confirmed supervisor authority: {}",
                        error.error
                    ),
                })
            }
        }
    }

    async fn pending_supervisor_acceptance_confirmed(
        &self,
        peer: &TrustedPeerDescriptor,
        pending: &crate::store::SupervisorAuthorityRecord,
        command: &super::bridge_protocol::BridgeCommand,
        protocol_version: super::bridge_protocol::BridgeProtocolVersion,
    ) -> Result<bool, MobError> {
        let value = self
            .supervisor_bridge
            .send_bridge_command_as_authority(
                pending,
                peer,
                command,
                std::time::Duration::from_secs(5),
            )
            .await?;
        if let Some(rejection) = Self::bridge_rejection_reply(protocol_version, &value) {
            // The shell extracts the pure typed wire cause; MobMachine owns the
            // acceptance/recovery verdict. A reply with no typed cause is failed
            // closed as `Fatal` (the historical `None` arm), bubbling the raw
            // rejection up without consulting the machine.
            let verdict = match rejection.typed_cause() {
                Some(cause) => self.classify_pending_supervisor_acceptance(cause)?,
                None => mob_dsl::MobPendingSupervisorAcceptanceKind::Fatal,
            };
            return match verdict {
                mob_dsl::MobPendingSupervisorAcceptanceKind::NotConfirmedReattempt => Ok(false),
                mob_dsl::MobPendingSupervisorAcceptanceKind::StalePendingAuthority => {
                    Err(Self::bridge_rejection_error_with_reason(
                        &rejection,
                        format!(
                            "pending authority is stale for peer '{}': {}",
                            peer.peer_id,
                            rejection.reason()
                        ),
                    ))
                }
                mob_dsl::MobPendingSupervisorAcceptanceKind::Fatal => {
                    Err(Self::bridge_rejection_error(rejection))
                }
            };
        }
        let _ack: super::bridge_protocol::BridgeAck =
            serde_json::from_value(value).map_err(|error| {
                MobError::Internal(format!(
                    "failed to decode pending supervisor verification response: {error}"
                ))
            })?;
        Ok(true)
    }

    async fn rotate_supervisor_bridge_to(
        &self,
        authority: &crate::store::SupervisorAuthorityRecord,
    ) -> Result<(), MobError> {
        if !self.supervisor_authority_record_is_machine_authorized(authority) {
            return Err(MobError::Internal(format!(
                "refusing to rotate supervisor bridge to peer={} epoch={} without generated MobMachine authority",
                authority.public_peer_id, authority.epoch
            )));
        }
        let active = self.supervisor_bridge.authority().await;
        if active.public_peer_id != authority.public_peer_id
            || active.epoch != authority.epoch
            || active.protocol_version != authority.protocol_version
        {
            let bridge_authority = self.supervisor_bridge_authority_for_record(authority)?;
            let prepared = self
                .supervisor_bridge
                .prepare_rotation(authority.clone(), &bridge_authority)?;
            self.supervisor_bridge
                .commit_prepared_rotation(prepared)
                .await;
        }
        Ok(())
    }

    async fn persist_pending_supervisor_rotation(
        &mut self,
        current: &crate::store::SupervisorAuthorityRecord,
        expected_durable: &crate::store::SupervisorAuthorityRecord,
        pending: &crate::store::SupervisorAuthorityRecord,
        accepted_peer_ids: &BTreeSet<String>,
    ) -> Result<SupervisorPendingRotationPersistence, MobError> {
        let mut record = current.without_pending_rotation();
        let pending_rotation = crate::store::SupervisorPendingRotationRecord::from_authority(
            pending,
            accepted_peer_ids.iter().cloned().collect(),
        );
        if !accepted_peer_ids.is_empty() {
            record.pending_rotation = Some(pending_rotation.clone());
        }
        let prepared = match record.pending_rotation.as_ref() {
            Some(pending) => self.prepare_supervisor_authority_persistence(
                record.dsl_record_pending_rotation_input(pending),
                &record,
                "persist_pending_supervisor_rotation",
            )?,
            None => self.prepare_supervisor_authority_persistence(
                record.dsl_clear_pending_rotation_input(),
                &record,
                "persist_pending_supervisor_rotation",
            )?,
        };
        match self
            .runtime_metadata
            .compare_and_put_supervisor_authority(
                &self.definition.id,
                expected_durable,
                &record,
                &prepared.authority,
            )
            .await
        {
            Ok(true) => {
                self.commit_prepared_dsl_transition(prepared.transition)?;
                Ok(SupervisorPendingRotationPersistence {
                    pending_authority_recorded: record.pending_rotation.is_some(),
                    persisted_record: Some(record),
                })
            }
            Ok(false) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    "supervisor authority changed while persisting pending rotation"
                );
                Ok(SupervisorPendingRotationPersistence {
                    pending_authority_recorded: false,
                    persisted_record: None,
                })
            }
            Err(error) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    error = %error,
                    "failed to persist pending supervisor rotation"
                );
                Ok(SupervisorPendingRotationPersistence {
                    pending_authority_recorded: false,
                    persisted_record: None,
                })
            }
        }
    }

    async fn rollback_rotated_supervisor_peers(
        &mut self,
        current: &crate::store::SupervisorAuthorityRecord,
        rotated_peers: &[(TrustedPeerDescriptor, crate::RuntimeBinding)],
    ) -> Result<(), MobError> {
        let current_sup_spec: super::bridge_protocol::BridgePeerSpec =
            Self::supervisor_spec_for_authority(&self.definition.id, current)?.into();
        let current_payload = super::bridge_protocol::BridgeSupervisorPayload {
            supervisor: current_sup_spec,
            epoch: current.epoch,
            protocol_version: current.protocol_version,
        };
        for (peer, binding) in rotated_peers {
            let authorized_bind = self
                .bind_peer_only_member_for_binding_with_payload(peer, binding, &current_payload)
                .await
                .map_err(|bind_error| {
                    MobError::WiringError(format!(
                        "failed to roll peer back to prior supervisor authority: {bind_error}"
                    ))
                })?;
            self.persist_rebound_binding(binding, &authorized_bind.peer, &authorized_bind.response)
                .await?;
        }
        Ok(())
    }

    fn rotated_peer_bindings_for_accepted_peer_ids(
        &self,
        remote_bindings: &[crate::RuntimeBinding],
        accepted_peer_ids: &BTreeSet<String>,
        rotated_peers: &[(TrustedPeerDescriptor, crate::RuntimeBinding)],
    ) -> Result<Vec<(TrustedPeerDescriptor, crate::RuntimeBinding)>, MobError> {
        let mut seen = BTreeSet::new();
        let mut peers = Vec::new();
        for (peer, binding) in rotated_peers {
            let peer_id = peer.peer_id.to_string();
            if accepted_peer_ids.contains(&peer_id) && seen.insert(peer_id) {
                peers.push((peer.clone(), binding.clone()));
            }
        }
        for binding in remote_bindings {
            let peer = Self::peer_only_spec_for_binding(
                binding,
                "rotated_peer_bindings_for_accepted_peer_ids",
            )?;
            let peer_id = peer.peer_id.to_string();
            if accepted_peer_ids.contains(&peer_id) && seen.insert(peer_id) {
                peers.push((peer, binding.clone()));
            }
        }
        Ok(peers)
    }

    async fn supervisor_reconciliation_authority(
        &mut self,
        _fallback: &crate::store::SupervisorAuthorityRecord,
    ) -> Result<crate::store::SupervisorAuthorityRecord, MobError> {
        let record = self
            .runtime_metadata
            .load_supervisor_authority(&self.definition.id)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "cannot reconcile supervisor authority for mob '{}': missing supervisor runtime metadata",
                    self.definition.id
                ))
            })?;
        let target = record.without_pending_rotation();
        if !self.supervisor_authority_record_is_machine_authorized(&target) {
            return Err(MobError::Internal(format!(
                "cannot reconcile supervisor authority for mob '{}': durable supervisor metadata is not authorized by the local MobMachine",
                self.definition.id
            )));
        }
        Ok(target)
    }

    async fn reconcile_attempted_supervisor_peers_to_authority(
        &mut self,
        attempted: &crate::store::SupervisorAuthorityRecord,
        target: &crate::store::SupervisorAuthorityRecord,
        rotated_peers: &[(TrustedPeerDescriptor, crate::RuntimeBinding)],
    ) -> Result<(), MobError> {
        if rotated_peers.is_empty() {
            self.rotate_supervisor_bridge_to(target).await?;
            return Ok(());
        }
        let target_payload = self.supervisor_payload_for_authority(target)?;
        let target_command =
            super::bridge_protocol::BridgeCommand::AuthorizeSupervisor(target_payload.clone());
        let attempted_payload = self.supervisor_payload_for_authority(attempted)?;
        let revoke_attempted_command =
            super::bridge_protocol::BridgeCommand::RevokeSupervisor(attempted_payload);
        for (peer, binding) in rotated_peers {
            let authorize_result = self
                .supervisor_bridge
                .send_bridge_command_as_authority(
                    attempted,
                    peer,
                    &target_command,
                    std::time::Duration::from_secs(5),
                )
                .await
                .and_then(|value| {
                    Self::bridge_ack_from_value(
                        target_command.protocol_version(),
                        value,
                        "supervisor reconciliation authorize",
                    )
                });
            if authorize_result.is_ok() {
                continue;
            }

            self.supervisor_bridge
                .send_bridge_command_as_authority(
                    attempted,
                    peer,
                    &revoke_attempted_command,
                    std::time::Duration::from_secs(5),
                )
                .await
                .and_then(|value| {
                    Self::bridge_ack_from_value(
                        revoke_attempted_command.protocol_version(),
                        value,
                        "supervisor reconciliation revoke",
                    )
                })?;
            self.rotate_supervisor_bridge_to(target).await?;
            let authorized_bind = self
                .bind_peer_only_member_for_binding_with_payload(peer, binding, &target_payload)
                .await?;
            self.persist_rebound_binding(binding, &authorized_bind.peer, &authorized_bind.response)
                .await?;
        }
        self.rotate_supervisor_bridge_to(target).await?;
        Ok(())
    }

    async fn recover_unrecorded_supervisor_attempt(
        &mut self,
        fallback_authority: &crate::store::SupervisorAuthorityRecord,
        attempted: &crate::store::SupervisorAuthorityRecord,
        accepted_peer_ids: &BTreeSet<String>,
        remote_bindings: &[crate::RuntimeBinding],
        rotated_peers: &[(TrustedPeerDescriptor, crate::RuntimeBinding)],
    ) -> Result<SupervisorUnrecordedAttemptRecovery, MobError> {
        let target = match self
            .supervisor_reconciliation_authority(fallback_authority)
            .await
        {
            Ok(target) => target,
            Err(error) => {
                return Ok(SupervisorUnrecordedAttemptRecovery {
                    rollback_succeeded: false,
                    rollback_error: Some(error.to_string()),
                });
            }
        };
        let peers_to_reconcile = self.rotated_peer_bindings_for_accepted_peer_ids(
            remote_bindings,
            accepted_peer_ids,
            rotated_peers,
        )?;
        let mut rollback_errors = Vec::new();
        if let Err(error) = self
            .reconcile_attempted_supervisor_peers_to_authority(
                attempted,
                &target,
                &peers_to_reconcile,
            )
            .await
        {
            rollback_errors.push(format!(
                "supervisor peer reconciliation failed after pending CAS conflict: {error}"
            ));
        }
        if let Err(error) = self.rotate_supervisor_bridge_to(&target).await {
            rollback_errors.push(format!(
                "supervisor bridge reconciliation to durable authority failed: {error}"
            ));
        }
        Ok(SupervisorUnrecordedAttemptRecovery {
            rollback_succeeded: rollback_errors.is_empty(),
            rollback_error: (!rollback_errors.is_empty()).then(|| rollback_errors.join("; ")),
        })
    }

    async fn activate_supervisor_authority(
        &mut self,
        current: &crate::store::SupervisorAuthorityRecord,
        expected_durable: &crate::store::SupervisorAuthorityRecord,
        next: &crate::store::SupervisorAuthorityRecord,
    ) -> Result<(), SupervisorAuthorityActivationError> {
        let prepared_commit = self
            .prepare_supervisor_authority_persistence(
                current.dsl_commit_rotation_input(next),
                next,
                "activate_supervisor_authority",
            )
            .map_err(|error| SupervisorAuthorityActivationError {
                error,
                rollback_succeeded: false,
                rollback_error: None,
            })?;
        let prepared_bridge_authority =
            crate::store::SupervisorAuthorityBridgeAuthority::from_persistence_authority(
                next,
                &prepared_commit.authority,
            )
            .map_err(|error| SupervisorAuthorityActivationError {
                error: MobError::from(error),
                rollback_succeeded: false,
                rollback_error: None,
            })?;
        let prepared_bridge_rotation = self
            .supervisor_bridge
            .prepare_rotation(next.clone(), &prepared_bridge_authority)
            .map_err(|error| SupervisorAuthorityActivationError {
                error,
                rollback_succeeded: false,
                rollback_error: None,
            })?;
        let previous_private_trust_removal_key = current.public_peer_id.clone();
        let session_member_refs = {
            let roster = self.roster.read().await;
            roster
                .list_all()
                .filter_map(|entry| match &entry.member_ref {
                    MemberRef::Session { .. } => Some(entry.member_ref.clone()),
                    MemberRef::BackendPeer { .. } => None,
                })
                .collect::<Vec<_>>()
        };
        let mut activated_session_trust: Vec<(SessionId, Arc<dyn CoreCommsRuntime>)> = Vec::new();
        for member_ref in session_member_refs {
            if let (Some(session_id), Some(comms)) = (
                member_ref.bridge_session_id().cloned(),
                self.provisioner_comms(&member_ref).await,
            ) {
                let supervisor_spec =
                    Self::supervisor_spec_for_authority(&self.definition.id, next).map_err(
                        |error| SupervisorAuthorityActivationError {
                            error,
                            rollback_succeeded: false,
                            rollback_error: None,
                        },
                    )?;
                match self
                    .install_supervisor_private_trust_for_session_authority(
                        &session_id,
                        &comms,
                        next,
                        supervisor_spec,
                        Some(&previous_private_trust_removal_key),
                    )
                    .await
                {
                    Ok(_) => activated_session_trust.push((session_id, comms)),
                    Err(error) => {
                        return Err(self
                            .supervisor_activation_error_with_rollback(
                                current,
                                &activated_session_trust,
                                error,
                            )
                            .await);
                    }
                }
            }
        }
        match self
            .runtime_metadata
            .compare_and_put_supervisor_authority(
                &self.definition.id,
                expected_durable,
                next,
                &prepared_commit.authority,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                let cas_error = MobError::from(crate::store::MobStoreError::CasConflict(format!(
                    "supervisor authority changed before final commit for mob '{}'",
                    self.definition.id
                )));
                return Err(self
                    .supervisor_activation_error_with_rollback(
                        current,
                        &activated_session_trust,
                        cas_error,
                    )
                    .await);
            }
            Err(error) => {
                return Err(self
                    .supervisor_activation_error_with_rollback(
                        current,
                        &activated_session_trust,
                        MobError::from(error),
                    )
                    .await);
            }
        }
        if let Err(error) = self.commit_prepared_dsl_transition(prepared_commit.transition) {
            return Err(self
                .supervisor_activation_error_with_rollback(current, &activated_session_trust, error)
                .await);
        }
        self.supervisor_bridge
            .commit_prepared_rotation(prepared_bridge_rotation)
            .await;
        Ok(())
    }

    async fn supervisor_activation_error_with_rollback(
        &self,
        current: &crate::store::SupervisorAuthorityRecord,
        activated_session_trust: &[(SessionId, Arc<dyn CoreCommsRuntime>)],
        error: MobError,
    ) -> SupervisorAuthorityActivationError {
        let mut rollback_errors = Vec::new();
        if let Err(rollback_error) = self.rotate_supervisor_bridge_to(current).await {
            rollback_errors.push(format!(
                "supervisor bridge rollback failed: {rollback_error}"
            ));
        } else {
            for (session_id, comms) in activated_session_trust.iter().rev() {
                if let Err(rollback_error) = self
                    .install_supervisor_private_trust_for_session(session_id, comms, None)
                    .await
                {
                    rollback_errors.push(format!(
                        "session '{session_id}' private trust rollback failed: {rollback_error}"
                    ));
                }
            }
        }
        let error_text = error.to_string();
        if error_text.contains("cleanup failed while removing new supervisor trust") {
            rollback_errors.push("supervisor private trust cleanup failed".to_string());
        }
        SupervisorAuthorityActivationError {
            error,
            rollback_succeeded: rollback_errors.is_empty(),
            rollback_error: (!rollback_errors.is_empty()).then(|| rollback_errors.join("; ")),
        }
    }

    /// Cancel checkpointers and transition to Stopped. Used by `handle_reset`
    /// error paths after destructive steps have already been taken.
    async fn fail_reset_to_stopped(&mut self) {
        self.provisioner.cancel_all_checkpointers().await;
        if let Err(e) =
            self.apply_dsl_input(mob_dsl::MobMachineInput::Stop, "fail_reset_to_stopped")
        {
            tracing::warn!(error = %e, "authority rejected Stop in fail_reset_to_stopped");
        }
    }

    async fn handle_reset(&mut self, prior_state: MobState) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_reset preflight")?;
        self.ensure_flow_tracker_alignment("handle_reset preflight")
            .await?;
        let was_stopped = prior_state == MobState::Stopped;
        self.cancel_all_flow_tasks().await?;

        // Rearm checkpointers temporarily so retire can checkpoint if needed.
        if was_stopped {
            self.provisioner.rearm_all_checkpointers().await;
        }

        // --- Destructive phase: retire members and stop MCP servers. ---
        // After this point the mob is effectively stopped regardless of what
        // the prior state field says.
        if let Err(error) = self.retire_all_members("reset").await {
            if was_stopped {
                self.provisioner.cancel_all_checkpointers().await;
            }
            return Err(error);
        }
        // ResetToRunning owns active/pending run counters and coordinator
        // binding. The durable epoch marker is appended only after the
        // generated authority has accepted the prepared transition.
        let prepared = match self
            .prepare_dsl_input_transition(mob_dsl::MobMachineInput::Reset, "reset_to_running")
            .map_err(|error| {
                tracing::debug!(
                    context = "reset_to_running",
                    error = %error,
                    "MobMachine command admission rejected input"
                );
                self.invalid_transition_to(MobState::Running)
            }) {
            Ok(prepared) => prepared,
            Err(error) => {
                if was_stopped {
                    self.provisioner.cancel_all_checkpointers().await;
                }
                return Err(error);
            }
        };
        if let Err(error) = Self::require_lifecycle_journal_effect(
            &prepared.transition,
            mob_dsl::MobLifecycleJournalKind::Reset,
            "reset_to_running",
        ) {
            if was_stopped {
                self.provisioner.cancel_all_checkpointers().await;
            }
            return Err(error);
        }

        // --- Event rewrite phase: append the new epoch marker. ---
        // Append-only epoch model: projections clear on MobReset; the original
        // MobCreated definition remains the durable resume authority for this
        // mob. No clear() needed -- crash-safe.
        let events = self.events.clone();
        let mob_id = self.definition.id.clone();
        if let Err(error) = self
            .commit_prepared_dsl_transition_after(prepared, move || async move {
                events
                    .append(NewMobEvent {
                        mob_id,
                        timestamp: None,
                        kind: MobEventKind::MobReset,
                    })
                    .await
                    .map_err(MobError::from)?;
                Ok(())
            })
            .await
        {
            self.fail_reset_to_stopped().await;
            return Err(error);
        }

        // Clear in-memory projections.
        self.edge_locks.clear().await;
        self.retired_event_index.write().await.clear();

        self.ensure_pending_spawn_alignment("handle_reset completion")?;
        self.ensure_flow_tracker_alignment("handle_reset completion")
            .await?;
        Ok(())
    }

    /// Retire all roster members in parallel (sliding window of
    /// `MAX_PARALLEL_REMOTE_MEMBER_TEARDOWNS`). handle_retire only returns Err on
    /// event-append failures (pre-cleanup); cleanup errors are best-effort.
    /// If any member fails to retire the operation is aborted — the caller
    /// can retry since already-retired members are idempotent.
    async fn retire_all_members(&mut self, context: &str) -> Result<(), MobError> {
        let prepared_retire_all = self.prepare_command_admission(
            mob_dsl::MobMachineInput::RetireAll,
            MobState::Running,
            context,
        )?;
        self.commit_prepared_dsl_input(prepared_retire_all)?;
        self.ensure_pending_spawn_alignment("retire_all_members preflight")?;
        let pending_reason = format!("{context}: draining pending spawns before bulk retirement");
        self.fail_all_pending_spawns(&pending_reason).await?;
        self.ensure_pending_spawn_alignment("retire_all_members after pending drain")?;
        self.cancel_pending_peer_deliveries("all members are retiring")
            .await;

        let ids = {
            let roster = self.roster.read().await;
            roster
                .list_all()
                .map(|entry| entry.agent_identity.clone())
                .collect::<Vec<_>>()
        };
        if ids.is_empty() {
            return Ok(());
        }

        let mut retire_failures: Vec<String> = Vec::new();
        for id in ids {
            let result = self.retire_one(id).await;
            if let Err((id, error)) = result {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %id,
                    error = %error,
                    "{context}: retire failed for member"
                );
                retire_failures.push(format!("{id}: {error}"));
            }
        }

        if !retire_failures.is_empty() {
            return Err(MobError::Internal(format!(
                "{context} aborted: {} member(s) could not be retired: {}",
                retire_failures.len(),
                retire_failures.join("; ")
            )));
        }
        self.ensure_pending_spawn_alignment("retire_all_members completion")?;
        Ok(())
    }

    async fn retire_one(&mut self, id: MeerkatId) -> Result<(), (MeerkatId, MobError)> {
        self.handle_retire_inner(&id, true, false)
            .await
            .map_err(|error| (id, error))
    }

    /// Unified work-lane entry.
    ///
    /// The `MobMachine` DSL owns work-origin legality: whether this runtime is
    /// live, which origin is admissible (External vs Internal), and whether
    /// external callers may address this runtime. The shell no longer
    /// re-decides any of those facts — it forwards the caller-declared
    /// [`WorkOrigin`] to the DSL and lets the guards accept or reject.
    ///
    /// Shell-owned pre-work (shell is the only place that can do these):
    ///   * Auto-spawn when the target member is absent and MobMachine accepts
    ///     typed spawn-policy resolution feedback. Only meaningful for
    ///     externally-originated work — internal origins never auto-spawn.
    ///   * Post-authorization dispatch — reading the machine's
    ///     generated `RequestRuntimeIngress` / `RequestPeerRuntimeIngress`
    ///     effect and materializing it as actual runtime ingress (event
    ///     injector or `StartTurnRequest`). The effect payload is the
    ///     authority token for the dispatch shape; the shell verifies it
    ///     before touching session or peer transport.
    async fn handle_submit_work(
        &mut self,
        payload: Box<super::state::SubmitWorkPayload>,
    ) -> Result<SubmitWorkDispatchCompletion, MobError> {
        let super::state::SubmitWorkPayload {
            runtime_id,
            fence_token,
            work_ref,
            content,
            origin,
            handling_mode,
            render_metadata,
            ack_mode,
        } = *payload;
        tracing::debug!(
            agent_identity = %runtime_id.identity,
            runtime_id = %runtime_id,
            work_ref = %work_ref,
            origin = ?origin,
            handling_mode = ?handling_mode,
            ack_mode = ?ack_mode,
            "handle_submit_work started"
        );
        self.ensure_pending_spawn_alignment("handle_submit_work preflight")?;

        let agent_identity = MeerkatId::from(&runtime_id.identity);
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let declared_dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&runtime_id);
        let declared_dsl_fence_token = mob_dsl::FenceToken::from_domain(fence_token);

        // SubmitWork admission belongs to MobMachine even when the shell may
        // need to auto-spawn an absent external target. Probe the declared
        // command before policy resolution so stopped/completed mobs reject
        // without staging spawn side effects.
        let declared_submit_work_admitted = match self.probe_command_admission(
            mob_dsl::MobMachineInput::SubmitWork {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: declared_dsl_runtime_id.clone(),
                fence_token: declared_dsl_fence_token,
                work_id: mob_dsl::WorkId::from_work_ref(&work_ref),
                origin: mob_dsl::WorkOrigin::from(origin),
            },
            MobState::Running,
            "submit_work_command_admission",
        ) {
            Ok(()) => true,
            Err(error) => {
                if self.state() != MobState::Running {
                    return Err(error);
                }
                false
            }
        };

        // Auto-spawn is an external-only policy seam that runs when the target
        // member is absent and MobMachine accepts typed spawn-policy feedback.
        // For existing members, the caller's runtime/fence pair is forwarded
        // into MobMachine so generated authority owns stale-incarnation
        // rejection.
        let initial_entry = {
            let roster = self.roster.read().await;
            roster.get(&agent_identity).cloned()
        };
        let initial_entry_present = initial_entry.is_some();
        let entry = match initial_entry {
            Some(e) => {
                self.ensure_member_not_broken(&e.agent_identity).await?;
                e
            }
            None => {
                if matches!(origin, WorkOrigin::Internal) {
                    let current_state = self.state();
                    return Err(Self::resolve_submit_work_projection_missing_or_rejection(
                        &mut self.dsl_authority,
                        declared_submit_work_admitted,
                        &dsl_identity,
                        &declared_dsl_runtime_id,
                        declared_dsl_fence_token,
                        &runtime_id,
                        origin,
                        &agent_identity,
                        current_state,
                    ));
                }
                let identity = AgentIdentity::from(agent_identity.as_str());
                if let Some(spec) = self.resolve_spawn_policy_via_machine(&identity).await? {
                    Box::pin(self.spawn_from_policy_inline(&identity, spec, &work_ref, origin))
                        .await?;
                    {
                        let roster = self.roster.read().await;
                        roster.get(&identity).cloned()
                    }
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "auto-spawned member '{identity}' missing from roster after completion"
                        ))
                    })?
                } else {
                    let current_state = self.state();
                    return Err(Self::resolve_submit_work_projection_missing_or_rejection(
                        &mut self.dsl_authority,
                        declared_submit_work_admitted,
                        &dsl_identity,
                        &declared_dsl_runtime_id,
                        declared_dsl_fence_token,
                        &runtime_id,
                        origin,
                        &agent_identity,
                        current_state,
                    ));
                }
            }
        };

        let admission_runtime_id = if initial_entry_present {
            runtime_id.clone()
        } else {
            entry.agent_runtime_id.clone()
        };
        let admission_fence_token = if initial_entry_present {
            fence_token
        } else {
            entry.fence_token
        };

        // Project the caller's identifiers into DSL bridging types. Existing
        // members use the caller-supplied runtime/fence so MobMachine can
        // reject stale generations; auto-spawned members use the generated
        // runtime/fence created by the spawn authority path.
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&admission_runtime_id);
        let dsl_fence_token = mob_dsl::FenceToken::from_domain(admission_fence_token);
        let dsl_work_id = mob_dsl::WorkId::from_work_ref(&work_ref);
        let dsl_origin = mob_dsl::WorkOrigin::from(origin);

        // Apply the DSL SubmitWork input. The MobMachine owns work-origin and
        // fence-token legality: `SubmitWorkRunningExternal` /
        // `SubmitWorkRunningInternal` encode origin, addressability,
        // live-runtime, fence-token, and phase guards.
        let transition = match mob_dsl::MobMachineMutator::apply(
            &mut self.dsl_authority,
            mob_dsl::MobMachineInput::SubmitWork {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: dsl_runtime_id.clone(),
                fence_token: dsl_fence_token,
                work_id: dsl_work_id.clone(),
                origin: dsl_origin,
            },
        ) {
            Ok(transition) => transition,
            Err(_) => {
                let current_state = self.state();
                return Err(Self::resolve_submit_work_rejection_in_authority(
                    &mut self.dsl_authority,
                    &dsl_identity,
                    &dsl_runtime_id,
                    dsl_fence_token,
                    &admission_runtime_id,
                    origin,
                    &agent_identity,
                    current_state,
                ));
            }
        };
        if transition.from_phase != transition.to_phase {
            let _ = self.phase_watch_tx.send(self.state());
        }
        self.publish_machine_state_projection();

        // The generated ingress effect is the authority token for realization.
        // Its payload must match the admitted work before the shell dispatches
        // to a session-bound runtime or a peer-only runtime.
        let ingress_authority = SubmitWorkIngressAuthority::from_transition(
            &transition,
            &dsl_runtime_id,
            dsl_fence_token,
            mob_dsl::Generation::from_domain(admission_runtime_id.generation),
            &dsl_work_id,
            dsl_origin,
        )?;
        drop(transition);

        let completion = self
            .dispatch_member_turn_after_machine_admission(
                &entry,
                ingress_authority,
                SubmitWorkDispatchRequest {
                    content,
                    handling_mode,
                    render_metadata,
                    ack_mode,
                    operation_id: None,
                },
            )
            .await?;
        tracing::debug!(
            agent_identity = %entry.agent_identity,
            runtime_id = %entry.agent_runtime_id,
            work_ref = %work_ref,
            handling_mode = ?handling_mode,
            ack_mode = ?ack_mode,
            completion = completion.kind(),
            "handle_submit_work dispatched after machine admission"
        );
        Ok(completion)
    }

    fn spawn_turn_completed_reply(
        provisioner: Arc<dyn MobProvisioner>,
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    ) {
        tokio::spawn(async move {
            let result = provisioner.start_turn(&member_ref, *req).await;
            let _ = reply_tx.send(result);
        });
    }

    fn spawn_turn_admission_reply(
        provisioner: Arc<dyn MobProvisioner>,
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    ) {
        tokio::spawn(async move {
            let result = provisioner.admit_turn(&member_ref, *req).await;
            let _ = reply_tx.send(result);
        });
    }

    async fn dispatch_turn_driven_spawn_initial_turn(
        &mut self,
        agent_identity: &MeerkatId,
        agent_runtime_id: &AgentRuntimeId,
        fence_token: FenceToken,
        operation_id: &meerkat_core::ops::OperationId,
        content: ContentInput,
    ) -> Result<(), MobError> {
        let entry = {
            let roster = self.roster.read().await;
            roster.get(agent_identity).cloned()
        }
        .ok_or_else(|| {
            MobError::Internal(format!(
                "turn-driven spawn initial SubmitWork for '{agent_identity}' had no roster projection after Spawn admission"
            ))
        })?;

        let work_ref = WorkRef::new();
        let origin = WorkOrigin::Internal;
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id);
        let dsl_fence_token = mob_dsl::FenceToken::from_domain(fence_token);
        let dsl_work_id = mob_dsl::WorkId::from_work_ref(&work_ref);
        let dsl_origin = mob_dsl::WorkOrigin::from(origin);
        let transition = match mob_dsl::MobMachineMutator::apply(
            &mut self.dsl_authority,
            mob_dsl::MobMachineInput::SubmitWork {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: dsl_runtime_id.clone(),
                fence_token: dsl_fence_token,
                work_id: dsl_work_id.clone(),
                origin: dsl_origin,
            },
        ) {
            Ok(transition) => transition,
            Err(_) => {
                let current_state = self.state();
                return Err(Self::resolve_submit_work_rejection_in_authority(
                    &mut self.dsl_authority,
                    &dsl_identity,
                    &dsl_runtime_id,
                    dsl_fence_token,
                    agent_runtime_id,
                    origin,
                    agent_identity,
                    current_state,
                ));
            }
        };
        if transition.from_phase != transition.to_phase {
            let _ = self.phase_watch_tx.send(self.state());
        }
        self.publish_machine_state_projection();
        let ingress_authority = SubmitWorkIngressAuthority::from_transition(
            &transition,
            &dsl_runtime_id,
            dsl_fence_token,
            mob_dsl::Generation::from_domain(agent_runtime_id.generation),
            &dsl_work_id,
            dsl_origin,
        )?;
        drop(transition);

        let completion = self
            .dispatch_member_turn_after_machine_admission(
                &entry,
                ingress_authority,
                SubmitWorkDispatchRequest {
                    content,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    render_metadata: None,
                    ack_mode: crate::mob_machine::SubmitWorkAckMode::IngressAccepted,
                    operation_id: Some(operation_id.clone()),
                },
            )
            .await?;
        tracing::debug!(
            agent_identity = %entry.agent_identity,
            runtime_id = %entry.agent_runtime_id,
            completion = completion.kind(),
            "dispatch_turn_driven_spawn_initial_turn dispatched after machine admission"
        );
        tracing::debug!(
            agent_identity = %entry.agent_identity,
            runtime_id = %entry.agent_runtime_id,
            "dispatch_turn_driven_spawn_initial_turn finishing dispatch"
        );
        self.finish_submit_work_dispatch(completion).await
    }

    /// Unified work-lane cancel entry.
    ///
    /// The MobMachine DSL `CancelAllWork` transition owns live-runtime
    /// membership, fence-token freshness, and phase legality. Once the machine
    /// accepts, the shell dispatches `interrupt_member` on the current bridge
    /// session.
    async fn handle_cancel_all_work(
        &mut self,
        runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
    ) -> Result<(), MobError> {
        let agent_identity = MeerkatId::from(&runtime_id.identity);
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let entry = {
            let roster = self.roster.read().await;
            roster.get(&agent_identity).cloned()
        };
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&runtime_id);
        let dsl_fence_token = mob_dsl::FenceToken::from_domain(fence_token);
        let prepared = self
            .prepare_dsl_input(
                mob_dsl::MobMachineInput::CancelAllWork {
                    agent_identity: dsl_identity.clone(),
                    agent_runtime_id: dsl_runtime_id.clone(),
                    fence_token: dsl_fence_token,
                },
                "handle_cancel_all_work",
            )
            .map_err(|_| {
                let current_state = self.state();
                Self::resolve_cancel_all_work_rejection_in_authority(
                    &mut self.dsl_authority,
                    &dsl_identity,
                    &dsl_runtime_id,
                    dsl_fence_token,
                    &runtime_id,
                    &agent_identity,
                    current_state,
                )
            })?;

        let entry = entry.ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine accepted CancelAllWork for '{agent_identity}' but the roster projection has no member entry"
            ))
        })?;

        // Feed the DSL CancelAllWork input. Guards have already accepted the
        // runtime binding, fence token, and phase.
        self.commit_prepared_dsl_input(prepared)?;

        // Dispatch the interrupt now that the machine has authorized.
        let machine_member_ref =
            self.machine_member_ref_for_behavior(&entry, "cancel all work interrupt")?;
        self.provisioner.interrupt_member(&machine_member_ref).await
    }

    async fn finish_submit_work_dispatch(
        &self,
        completion: SubmitWorkDispatchCompletion,
    ) -> Result<(), MobError> {
        match completion {
            SubmitWorkDispatchCompletion::Completed => {
                tracing::debug!("finish_submit_work_dispatch completed without runtime call");
                Ok(())
            }
            SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                operation_id,
                member_ref,
                req,
            } => {
                tracing::debug!(
                    member_ref = ?member_ref,
                    operation_id = ?operation_id,
                    "finish_submit_work_dispatch admitting turn"
                );
                let result = if let Some(operation_id) = operation_id.as_ref() {
                    self.provisioner
                        .admit_turn_for_operation(&member_ref, operation_id, *req)
                        .await
                } else {
                    self.provisioner.admit_turn(&member_ref, *req).await
                };
                tracing::debug!(
                    member_ref = ?member_ref,
                    ok = result.is_ok(),
                    "finish_submit_work_dispatch admitted turn"
                );
                result
            }
            SubmitWorkDispatchCompletion::AwaitTurnCompletion { member_ref, req } => {
                tracing::debug!(
                    member_ref = ?member_ref,
                    "finish_submit_work_dispatch starting turn"
                );
                let result = self.provisioner.start_turn(&member_ref, *req).await;
                tracing::debug!(
                    member_ref = ?member_ref,
                    ok = result.is_ok(),
                    "finish_submit_work_dispatch started turn"
                );
                result
            }
        }
    }

    async fn dispatch_member_turn_after_machine_admission(
        &mut self,
        entry: &RosterEntry,
        ingress_authority: SubmitWorkIngressAuthority,
        request: SubmitWorkDispatchRequest,
    ) -> Result<SubmitWorkDispatchCompletion, MobError> {
        let SubmitWorkDispatchRequest {
            content,
            handling_mode,
            render_metadata,
            ack_mode,
            operation_id,
        } = request;
        tracing::debug!(
            agent_identity = %entry.agent_identity,
            runtime_id = %entry.agent_runtime_id,
            runtime_mode = ?entry.runtime_mode,
            handling_mode = ?handling_mode,
            ack_mode = ?ack_mode,
            ingress_authority = ingress_authority.variant(),
            "dispatch_member_turn_after_machine_admission started"
        );
        let live_steer_admission = handling_mode == meerkat_core::types::HandlingMode::Steer
            && ack_mode == crate::mob_machine::SubmitWorkAckMode::IngressAccepted;
        tracing::debug!(
            agent_identity = %entry.agent_identity,
            "dispatch_member_turn_after_machine_admission resolving member ref"
        );
        let machine_member_ref =
            self.machine_member_ref_for_behavior(entry, "direct turn delivery")?;
        tracing::debug!(
            agent_identity = %entry.agent_identity,
            member_ref = ?machine_member_ref,
            "dispatch_member_turn_after_machine_admission resolved member ref"
        );
        ingress_authority.verify_member_ref(&machine_member_ref, "direct turn delivery")?;
        tracing::debug!(
            agent_identity = %entry.agent_identity,
            "dispatch_member_turn_after_machine_admission verified ingress authority"
        );
        if !live_steer_admission
            && let Some(bridge_session_id) = machine_member_ref.bridge_session_id()
        {
            tracing::debug!(
                agent_identity = %entry.agent_identity,
                session_id = %bridge_session_id,
                "dispatch_member_turn_after_machine_admission checking live session"
            );
            match self
                .session_service
                .has_live_session(bridge_session_id)
                .await
            {
                Ok(true) => {
                    tracing::debug!(
                        agent_identity = %entry.agent_identity,
                        session_id = %bridge_session_id,
                        "dispatch_member_turn_after_machine_admission live session exists"
                    );
                }
                Ok(false) | Err(meerkat_core::service::SessionError::NotFound { .. }) => {
                    let reason = self
                        .record_missing_member_bridge_session(
                            &entry.agent_identity,
                            bridge_session_id,
                            "dispatch_member_turn",
                        )
                        .await
                        .unwrap_or_else(|| {
                            format!("missing bridge session snapshot for '{bridge_session_id}'")
                        });
                    return Err(MobError::MemberRestoreFailed {
                        member_id: entry.agent_identity.clone(),
                        session_id: Some(bridge_session_id.clone()),
                        reason,
                    });
                }
                Err(error) => return Err(MobError::SessionError(error)),
            }
        }
        tracing::debug!(
            agent_identity = %entry.agent_identity,
            runtime_mode = ?entry.runtime_mode,
            "dispatch_member_turn_after_machine_admission dispatching runtime mode"
        );

        match entry.runtime_mode {
            crate::MobRuntimeMode::AutonomousHost => {
                if ingress_authority.is_peer_runtime() {
                    return Err(MobError::Internal(format!(
                        "autonomous direct turn delivery requires generated RequestRuntimeIngress authority for '{}'",
                        entry.agent_identity
                    )));
                }
                let bridge_session_id = machine_member_ref
                    .bridge_session_id()
                    .cloned()
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "autonomous direct turn delivery requires MobMachine session binding for '{}'",
                            entry.agent_identity
                        ))
                    })?;

                self.ensure_autonomous_runtime_ready(&entry.agent_identity, &machine_member_ref)
                    .await?;

                if self
                    .autonomous_steer_requires_admission_barrier(
                        entry,
                        &machine_member_ref,
                        handling_mode,
                        ack_mode,
                    )
                    .await?
                {
                    let req = meerkat_core::service::StartTurnRequest {
                        prompt: content,
                        system_prompt: None,
                        event_tx: None,
                        runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                            render_metadata,
                            handling_mode,
                            None,
                            None,
                            Vec::new(),
                            None,
                        ),
                    };
                    return Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                        operation_id,
                        member_ref: machine_member_ref,
                        req: Box::new(req),
                    });
                }
                let injector = self
                    .provisioner
                    .interaction_event_injector(&bridge_session_id)
                    .await
                    .ok_or_else(|| MobError::MissingMemberCapability {
                        member_id: crate::ids::MeerkatId::from(entry.agent_identity.as_str()),
                        capability: crate::error::MobMemberCapability::InteractionEventInjector,
                        context: "autonomous direct turn delivery",
                    })?;
                injector
                    .inject(
                        content,
                        meerkat_core::PlainEventSource::Rpc,
                        handling_mode,
                        render_metadata,
                    )
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "autonomous dispatch inject failed for '{}': {}",
                            entry.agent_identity, error
                        ))
                    })?;
                Ok(SubmitWorkDispatchCompletion::Completed)
            }
            crate::MobRuntimeMode::TurnDriven => {
                tracing::debug!(
                    agent_identity = %entry.agent_identity,
                    ingress_is_peer_runtime = ingress_authority.is_peer_runtime(),
                    "dispatch_member_turn_after_machine_admission entering turn-driven dispatch"
                );
                let machine_member_ref = if ingress_authority.is_peer_runtime() {
                    self.authorize_peer_only_member_ref_for_behavior(
                        &machine_member_ref,
                        "turn-driven direct turn delivery",
                    )
                    .await?
                } else {
                    machine_member_ref
                };
                tracing::debug!(
                    agent_identity = %entry.agent_identity,
                    member_ref = ?machine_member_ref,
                    "dispatch_member_turn_after_machine_admission building turn request"
                );
                let req = meerkat_core::service::StartTurnRequest {
                    prompt: content,
                    system_prompt: None,
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        render_metadata,
                        handling_mode,
                        None,
                        None,
                        Vec::new(),
                        None,
                    ),
                };
                tracing::debug!(
                    agent_identity = %entry.agent_identity,
                    ack_mode = ?ack_mode,
                    "dispatch_member_turn_after_machine_admission built turn request"
                );
                match ack_mode {
                    crate::mob_machine::SubmitWorkAckMode::IngressAccepted => {
                        tracing::debug!(
                            agent_identity = %entry.agent_identity,
                            "dispatch_member_turn_after_machine_admission boxing turn admission request"
                        );
                        let req = Box::new(req);
                        tracing::debug!(
                            agent_identity = %entry.agent_identity,
                            "dispatch_member_turn_after_machine_admission boxed turn admission request"
                        );
                        Ok(SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                            operation_id,
                            member_ref: machine_member_ref,
                            req,
                        })
                    }
                    crate::mob_machine::SubmitWorkAckMode::TurnCompleted => {
                        tracing::debug!(
                            agent_identity = %entry.agent_identity,
                            "dispatch_member_turn_after_machine_admission boxing turn completion request"
                        );
                        let req = Box::new(req);
                        tracing::debug!(
                            agent_identity = %entry.agent_identity,
                            "dispatch_member_turn_after_machine_admission boxed turn completion request"
                        );
                        Ok(SubmitWorkDispatchCompletion::AwaitTurnCompletion {
                            member_ref: machine_member_ref,
                            req,
                        })
                    }
                }
            }
        }
    }

    async fn autonomous_steer_requires_admission_barrier(
        &self,
        entry: &RosterEntry,
        member_ref: &MemberRef,
        handling_mode: meerkat_core::types::HandlingMode,
        ack_mode: crate::mob_machine::SubmitWorkAckMode,
    ) -> Result<bool, MobError> {
        if handling_mode != meerkat_core::types::HandlingMode::Steer
            || ack_mode != crate::mob_machine::SubmitWorkAckMode::IngressAccepted
        {
            return Ok(false);
        }

        #[cfg(feature = "runtime-adapter")]
        if let (Some(adapter), Some(session_id)) =
            (&self.runtime_adapter, member_ref.bridge_session_id())
        {
            use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

            match adapter.runtime_state(session_id).await {
                Ok(meerkat_runtime::RuntimeState::Running) => {
                    tracing::debug!(
                        agent_identity = %entry.agent_identity,
                        session_id = %session_id,
                        "active steer admission barrier enabled by running runtime state"
                    );
                    return Ok(true);
                }
                Ok(state) => {
                    let session_active = self
                        .provisioner
                        .is_member_active(member_ref)
                        .await?
                        .unwrap_or(false);
                    tracing::debug!(
                        agent_identity = %entry.agent_identity,
                        session_id = %session_id,
                        runtime_state = ?state,
                        session_active,
                        "active steer admission barrier checked non-running runtime state"
                    );
                    if session_active {
                        return Ok(true);
                    }
                    return Ok(false);
                }
                Err(error) => {
                    tracing::debug!(
                        agent_identity = %entry.agent_identity,
                        session_id = %session_id,
                        error = %error,
                        "runtime state unavailable while checking autonomous steer admission barrier"
                    );
                    return Ok(false);
                }
            }
        }

        Ok(false)
    }

    async fn handle_run_flow(
        &mut self,
        flow_id: FlowId,
        activation_params: serde_json::Value,
        scoped_event_tx: Option<mpsc::Sender<meerkat_core::ScopedAgentEvent>>,
    ) -> Result<RunId, MobError> {
        self.ensure_pending_spawn_alignment("handle_run_flow preflight")?;
        self.ensure_flow_tracker_alignment("handle_run_flow preflight")
            .await?;
        let run_id = RunId::new();
        self.preview_run_flow_command_admission(&run_id)?;
        let config = FlowRunConfig::from_definition(flow_id, &self.definition)?;
        let run_flow = MobRun::run_flow_input(&run_id, &config)?;
        debug_assert!(matches!(run_flow, mob_dsl::MobMachineInput::RunFlow { .. }));
        let prepared_run_flow = self
            .prepare_dsl_input(run_flow.clone(), "run_flow")
            .map_err(|_| self.invalid_transition_to(MobState::Running))?;
        self.create_pending_run(
            run_id.clone(),
            &config,
            prepared_run_flow.authority.state(),
            activation_params.clone(),
            vec![run_flow],
        )
        .await
        .inspect_err(|error| {
            tracing::warn!(
                run_id = %run_id,
                flow_id = %config.flow_id,
                error = %error,
                "flow admission run-store create failed before committing MobMachine RunFlow"
            );
        })?;
        self.commit_prepared_dsl_input(prepared_run_flow)?;
        if let Err(error) = self.apply_dsl_signal(mob_dsl::MobMachineSignal::StartRun, "start_run")
        {
            let mut details = Vec::new();
            if let Err(rollback_error) = self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::CompleteFlow,
                "complete_flow_rollback",
            ) {
                details.push(format!(
                    "RunFlow CompleteFlow rollback failed: {rollback_error}"
                ));
            }
            let terminalize_reason =
                format!("lifecycle StartRun transition failed during flow admission: {error}");
            if let Err(terminalize_error) = self
                .terminalize_failed_in_actor(
                    run_id.clone(),
                    config.flow_id.clone(),
                    terminalize_reason,
                    "run_flow_start_signal_terminalize_failed",
                )
                .await
            {
                details.push(format!(
                    "terminalizing pending run failed: {terminalize_error}"
                ));
            }
            let detail_suffix = if details.is_empty() {
                String::new()
            } else {
                format!("; {}", details.join("; "))
            };
            return Err(MobError::Internal(format!(
                "lifecycle StartRun transition failed during flow admission: {error}{detail_suffix}"
            )));
        }

        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.run_cancel_tokens.insert(
            run_id.clone(),
            (cancel_token.clone(), config.flow_id.clone()),
        );
        if let Some(scoped_event_tx) = scoped_event_tx {
            self.flow_streams
                .lock()
                .await
                .insert(run_id.clone(), scoped_event_tx);
        }

        let engine = self.flow_engine.clone();
        let cleanup_tx = self.command_tx.clone();
        let flow_engine = self.flow_engine.clone();
        let flow_run_id = run_id.clone();
        let flow_id_for_task = config.flow_id.clone();
        let cleanup_run_id = run_id.clone();
        let handle = tokio::spawn(async move {
            let run_id_for_execute = flow_run_id.clone();
            let execution_cancel = cancel_token.clone();
            if let Err(error) = engine
                .execute_flow(
                    run_id_for_execute,
                    config,
                    activation_params,
                    execution_cancel,
                )
                .await
            {
                tracing::error!(
                    run_id = %flow_run_id,
                    flow_id = %flow_id_for_task,
                    error = %error,
                    "flow task execution failed; delegating terminalization to flow-run kernel"
                );
                if cancel_token.is_cancelled() {
                    if let Err(finalize_error) = flow_engine
                        .terminalize_canceled(flow_run_id.clone(), flow_id_for_task)
                        .await
                    {
                        tracing::error!(
                            run_id = %flow_run_id,
                            error = %finalize_error,
                            "failed to finalize canceled run after flow task cancellation"
                        );
                    }
                } else {
                    match error {
                        MobError::RunCanceled(_) => {
                            if let Err(finalize_error) = flow_engine
                                .terminalize_canceled(flow_run_id.clone(), flow_id_for_task)
                                .await
                            {
                                tracing::error!(
                                    run_id = %flow_run_id,
                                    error = %finalize_error,
                                    "failed to finalize canceled run after flow task cancellation"
                                );
                            }
                        }
                        other => {
                            if let Err(finalize_error) = flow_engine
                                .terminalize_failed(
                                    flow_run_id.clone(),
                                    flow_id_for_task,
                                    other.to_string(),
                                )
                                .await
                            {
                                tracing::error!(
                                    run_id = %flow_run_id,
                                    error = %finalize_error,
                                    "failed to finalize run after flow task error"
                                );
                            }
                        }
                    }
                }
            }
            if cleanup_tx
                .send(MobCommand::FlowFinished {
                    run_id: cleanup_run_id,
                })
                .await
                .is_err()
            {
                tracing::warn!(
                    run_id = %flow_run_id,
                    "failed to send FlowFinished cleanup command"
                );
            }
        });
        self.run_tasks.insert(run_id.clone(), handle);
        self.ensure_flow_tracker_alignment("handle_run_flow completion")
            .await?;

        Ok(run_id)
    }

    async fn create_pending_run(
        &self,
        run_id: RunId,
        config: &FlowRunConfig,
        machine_state: &mob_dsl::MobMachineState,
        activation_params: serde_json::Value,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<RunId, MobError> {
        let flow_state =
            MobRun::flow_state_for_config_with_authority(&run_id, config, machine_state)?;
        let mut run = MobRun::pending_with_run_id(
            run_id.clone(),
            self.definition.id.clone(),
            config.flow_id.clone(),
            flow_state,
            activation_params,
        );
        run.append_flow_authority_inputs(authority_inputs)?;
        self.run_store.create_run(run).await?;
        Ok(run_id)
    }

    async fn handle_flow_cleanup(
        &mut self,
        run_id: RunId,
        context: &'static str,
    ) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_flow_cleanup preflight")?;
        self.ensure_flow_tracker_alignment("handle_flow_cleanup preflight")
            .await?;
        let has_task = self.run_tasks.contains_key(&run_id);
        let has_token = self.run_cancel_tokens.contains_key(&run_id);
        let has_stream = self.flow_streams.lock().await.contains_key(&run_id);

        if !has_task && !has_token && !has_stream {
            let run_terminal = self
                .run_store
                .get_run(&run_id)
                .await?
                .as_ref()
                .map(|run| crate::run::mob_machine_run_status_is_terminal(&run.run_id, &run.status))
                .transpose()?
                .unwrap_or(false);
            if !run_terminal {
                return Err(MobError::Internal(format!(
                    "{context}: received cleanup for run {run_id} with no local trackers before the persisted run reached a terminal status"
                )));
            }
            tracing::debug!(
                run_id = %run_id,
                context = context,
                "flow cleanup command had no local run-tracker entries"
            );
            return Ok(());
        }

        self.apply_dsl_signal(mob_dsl::MobMachineSignal::CompleteFlow, "flow_cleanup")?;
        self.apply_dsl_signal(mob_dsl::MobMachineSignal::FinishRun, "flow_cleanup")?;

        let _ = self.run_tasks.remove(&run_id);
        let _ = self.run_cancel_tokens.remove(&run_id);
        let _ = self.flow_streams.lock().await.remove(&run_id);
        self.ensure_flow_tracker_alignment("handle_flow_cleanup completion")
            .await?;
        Ok(())
    }

    async fn handle_cancel_flow(&mut self, run_id: RunId) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_cancel_flow preflight")?;
        self.apply_command_admission(
            mob_dsl::MobMachineInput::CancelFlow {
                run_id: mob_dsl::RunId::from(run_id.to_string()),
            },
            MobState::Running,
            "cancel_flow",
        )?;
        let Some((cancel_token, flow_id)) = self
            .run_cancel_tokens
            .get(&run_id)
            .map(|(token, flow_id)| (token.clone(), flow_id.clone()))
        else {
            if self.run_tasks.contains_key(&run_id)
                || self.flow_streams.lock().await.contains_key(&run_id)
            {
                return Err(MobError::Internal(format!(
                    "handle_cancel_flow: run {run_id} missing cancel token despite live task/stream trackers"
                )));
            }
            self.ensure_flow_tracker_alignment("handle_cancel_flow no-op completion")
                .await?;
            return Ok(());
        };
        self.flow_streams.lock().await.remove(&run_id);
        cancel_token.cancel();

        let Some(mut handle) = self.run_tasks.remove(&run_id) else {
            self.flow_engine.cancel_unfinished_steps(&run_id).await?;
            self.terminalize_canceled_in_actor(
                run_id.clone(),
                flow_id,
                "cancel_flow_no_handle_terminalize_canceled",
            )
            .await?;
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::CompleteFlow, "cancel_flow_no_handle")
                .map_err(|error| {
                    MobError::Internal(format!(
                        "flow canceled cleanup (no task handle): lifecycle CompleteFlow transition failed for run {run_id}: {error}"
                    ))
                })?;
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::FinishRun, "cancel_flow_no_handle")
                .map_err(|error| {
                    MobError::Internal(format!(
                        "flow canceled cleanup (no task handle): lifecycle FinishRun transition failed for run {run_id}: {error}"
                    ))
                })?;
            let _ = self.run_cancel_tokens.remove(&run_id);
            self.ensure_flow_tracker_alignment("handle_cancel_flow no-task cleanup")
                .await?;
            return Ok(());
        };

        let flow_engine = self.flow_engine.clone();
        let cleanup_tx = self.command_tx.clone();
        let cancel_grace_timeout = self
            .definition
            .limits
            .as_ref()
            .and_then(|limits| limits.cancel_grace_timeout_ms)
            .map_or_else(
                || std::time::Duration::from_secs(5),
                std::time::Duration::from_millis,
            );
        let cleanup_run_tracker = run_id.clone();
        let cleanup_run_id_for_error = run_id.clone();
        let cleanup_handle = tokio::spawn(async move {
            let cleanup_run_id = run_id.clone();
            let completed = tokio::select! {
                _ = &mut handle => true,
                () = tokio::time::sleep(cancel_grace_timeout) => false,
            };
            if completed {
                if let Err(error) = flow_engine.cancel_unfinished_steps(&run_id).await {
                    tracing::error!(
                        error = %error,
                        "failed to settle dispatched steps after flow task completion during cancellation"
                    );
                }
                if let Err(error) = flow_engine
                    .terminalize_canceled(run_id.clone(), flow_id)
                    .await
                {
                    tracing::error!(
                        error = %error,
                        "failed to apply canceled terminalization after flow task completion"
                    );
                }
                if cleanup_tx
                    .send(MobCommand::FlowCanceledCleanup {
                        run_id: cleanup_run_id,
                    })
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "failed to send FlowCanceledCleanup command after task completion"
                    );
                }
                return;
            }

            handle.abort();
            if let Err(error) = flow_engine.cancel_unfinished_steps(&run_id).await {
                tracing::error!(
                    error = %error,
                    "failed to settle dispatched steps before flow cancellation terminalization"
                );
            }
            if let Err(error) = flow_engine.terminalize_canceled(run_id, flow_id).await {
                tracing::error!(
                    error = %error,
                    "failed flow-run kernel cancellation terminalization"
                );
            }
            if cleanup_tx
                .send(MobCommand::FlowCanceledCleanup {
                    run_id: cleanup_run_id,
                })
                .await
                .is_err()
            {
                tracing::warn!("failed to send FlowCanceledCleanup command");
            }
        });
        if let Some(replaced) = self.run_tasks.insert(cleanup_run_tracker, cleanup_handle) {
            replaced.abort();
            return Err(MobError::Internal(format!(
                "handle_cancel_flow: duplicate flow cleanup task registration for run {cleanup_run_id_for_error}"
            )));
        }
        self.ensure_flow_tracker_alignment("handle_cancel_flow completion")
            .await?;

        Ok(())
    }

    async fn apply_flow_run_command_in_actor(
        &mut self,
        run_id: &RunId,
        command: MobMachineFlowRunCommand,
        context: &'static str,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        let authority_input = command.authority_input(run_id);
        let prepared = self.prepare_dsl_input(authority_input.clone(), context)?;
        let machine_state = prepared.authority.state().clone();
        let machine_effects = prepared.effects.clone();
        let authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        let effects = if matches!(command, MobMachineFlowRunCommand::StartRun(_)) {
            self.flow_engine
                .start_run_state_with_machine_state(
                    run_id,
                    machine_state,
                    authority,
                    machine_effects,
                    context,
                )
                .await?
        } else {
            Some(
                self.flow_engine
                    .apply_command_with_machine_state(
                        run_id,
                        command,
                        machine_state,
                        authority,
                        machine_effects,
                        context,
                    )
                    .await?,
            )
        };
        if effects.is_some() {
            self.commit_prepared_dsl_input(prepared)?;
        }
        Ok(effects)
    }

    async fn commit_flow_run_command_in_actor(
        &mut self,
        run_id: &RunId,
        command: MobMachineFlowRunCommand,
        context: &'static str,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.apply_flow_run_command_in_actor(run_id, command, context)
            .await
    }

    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        if !self
            .flow_frame_store_plan_expected_matches(run_id, &plan)
            .await?
        {
            return Ok(false);
        }
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let authority_inputs = plan.machine_inputs().to_vec();
        let won = match &plan {
            FlowFrameLoopStorePlan::InsertFrame {
                frame_id,
                initial_frame,
                ..
            } => self
                .run_store
                .cas_frame_state_with_authority(
                    run_id,
                    frame_id,
                    None,
                    initial_frame.clone(),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::FrameState {
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_frame_state_with_authority(
                    run_id,
                    frame_id,
                    Some(expected_frame),
                    next_frame.clone(),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::CompleteStepAndRecordOutput {
                frame_id,
                expected_frame,
                next_frame,
                step_output_key,
                step_output,
                loop_context,
                ..
            } => self
                .run_store
                .cas_complete_step_and_record_output_with_authority(
                    run_id,
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    step_output_key.clone(),
                    step_output.clone(),
                    loop_context
                        .as_ref()
                        .map(|(loop_id, iteration)| (loop_id, *iteration)),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::GrantNodeSlot {
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_grant_node_slot_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::StartLoop {
                loop_instance_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                initial_loop,
                ..
            } => self
                .run_store
                .cas_start_loop_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_run_state,
                    next_run_state.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    initial_loop.clone(),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::GrantBodyFrameStart {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                initial_frame,
                ledger_entry,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_grant_body_frame_start_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    initial_frame.clone(),
                    ledger_entry.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::SealFrame {
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_frame_state_with_authority(
                    run_id,
                    frame_id,
                    Some(expected_frame),
                    next_frame.clone(),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::CompleteBodyFrame {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_complete_body_frame_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::LoopRequestBodyFrame {
                loop_instance_id,
                expected_loop,
                next_loop,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_loop_request_body_frame_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::CompleteLoop {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_complete_loop_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                    authority_inputs,
                )
                .await
                .map_err(MobError::from)?,
        };
        if won {
            self.commit_prepared_dsl_input(prepared)?;
        }
        Ok(won)
    }

    async fn flow_frame_store_plan_expected_matches(
        &self,
        run_id: &RunId,
        plan: &FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        Ok(match plan {
            FlowFrameLoopStorePlan::InsertFrame { frame_id, .. } => {
                !run.frames.contains_key(frame_id)
            }
            FlowFrameLoopStorePlan::FrameState {
                frame_id,
                expected_frame,
                ..
            }
            | FlowFrameLoopStorePlan::CompleteStepAndRecordOutput {
                frame_id,
                expected_frame,
                ..
            }
            | FlowFrameLoopStorePlan::SealFrame {
                frame_id,
                expected_frame,
                ..
            } => run.frames.get(frame_id) == Some(expected_frame),
            FlowFrameLoopStorePlan::GrantNodeSlot {
                expected_run_state,
                frame_id,
                expected_frame,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.frames.get(frame_id) == Some(expected_frame)
            }
            FlowFrameLoopStorePlan::StartLoop {
                loop_instance_id,
                expected_run_state,
                frame_id,
                expected_frame,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.frames.get(frame_id) == Some(expected_frame)
                    && !run.loops.contains_key(loop_instance_id)
            }
            FlowFrameLoopStorePlan::GrantBodyFrameStart {
                loop_instance_id,
                expected_loop,
                frame_id,
                expected_run_state,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.loops.get(loop_instance_id) == Some(expected_loop)
                    && !run.frames.contains_key(frame_id)
            }
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state, ..
            } => &run.flow_state == expected_run_state,
            FlowFrameLoopStorePlan::CompleteBodyFrame {
                loop_instance_id,
                expected_loop,
                frame_id,
                expected_frame,
                expected_run_state,
                ..
            }
            | FlowFrameLoopStorePlan::CompleteLoop {
                loop_instance_id,
                expected_loop,
                frame_id,
                expected_frame,
                expected_run_state,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.loops.get(loop_instance_id) == Some(expected_loop)
                    && run.frames.get(frame_id) == Some(expected_frame)
            }
            FlowFrameLoopStorePlan::LoopRequestBodyFrame {
                loop_instance_id,
                expected_loop,
                expected_run_state,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.loops.get(loop_instance_id) == Some(expected_loop)
            }
        })
    }

    async fn commit_flow_terminalization_in_actor(
        &mut self,
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
        command: MobMachineFlowRunCommand,
        context: &'static str,
    ) -> Result<TerminalizationOutcome, MobError> {
        let run = self
            .run_store
            .get_run(&run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        if crate::run::mob_machine_run_status_is_terminal(&run.run_id, &run.status)?
            && !matches!(
                (&target, &run.status),
                (TerminalizationTarget::Canceled, MobRunStatus::Failed)
            )
        {
            return self
                .flow_engine
                .repair_persisted_terminalization(run_id, flow_id, target)
                .await;
        }

        let authority_input = command.authority_input(&run_id);
        let prepared = self.prepare_dsl_input(authority_input.clone(), context)?;
        let machine_state = prepared.authority.state().clone();
        let machine_effects = prepared.effects.clone();
        let authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        let outcome = self
            .flow_engine
            .terminalize_with_machine_state(
                run_id.clone(),
                flow_id,
                target.clone(),
                command,
                machine_state,
                authority,
                machine_effects,
            )
            .await;
        match outcome {
            Ok(TerminalizationOutcome::Transitioned) => {
                self.commit_prepared_dsl_input(prepared)?;
                Ok(TerminalizationOutcome::Transitioned)
            }
            Ok(TerminalizationOutcome::Noop) => Ok(TerminalizationOutcome::Noop),
            Err(error) => {
                if self
                    .persisted_terminal_status_matches_target(&run_id, &target)
                    .await?
                {
                    self.commit_prepared_dsl_input(prepared)?;
                }
                Err(error)
            }
        }
    }

    async fn persisted_terminal_status_matches_target(
        &self,
        run_id: &RunId,
        target: &TerminalizationTarget,
    ) -> Result<bool, MobError> {
        Ok(self
            .run_store
            .get_run(run_id)
            .await?
            .is_some_and(|run| run.status == target.status()))
    }

    async fn cancel_unfinished_steps_in_actor(&mut self, run_id: &RunId) -> Result<(), MobError> {
        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        for step_id in run.ordered_steps()? {
            if run
                .flow_state
                .step_status
                .get(&step_id)
                .and_then(|status| *status)
                .is_some_and(|status| !matches!(status, flow_run::StepRunStatus::Dispatched))
            {
                continue;
            }
            self.apply_flow_run_command_in_actor(
                run_id,
                MobMachineFlowRunCommand::CancelStep(flow_run::inputs::CancelStep { step_id }),
                "actor_cancel_unfinished_step",
            )
            .await?;
        }
        Ok(())
    }

    async fn terminalize_canceled_in_actor(
        &mut self,
        run_id: RunId,
        flow_id: FlowId,
        context: &'static str,
    ) -> Result<(), MobError> {
        let _ = self
            .commit_flow_terminalization_in_actor(
                run_id,
                flow_id,
                TerminalizationTarget::Canceled,
                MobMachineFlowRunCommand::TerminalizeCanceled(
                    flow_run::inputs::TerminalizeCanceled {},
                ),
                context,
            )
            .await?;
        Ok(())
    }

    async fn terminalize_failed_in_actor(
        &mut self,
        run_id: RunId,
        flow_id: FlowId,
        reason: String,
        context: &'static str,
    ) -> Result<(), MobError> {
        let _ = self
            .commit_flow_terminalization_in_actor(
                run_id,
                flow_id,
                TerminalizationTarget::Failed { reason },
                MobMachineFlowRunCommand::TerminalizeFailed(flow_run::inputs::TerminalizeFailed {}),
                context,
            )
            .await?;
        Ok(())
    }

    async fn cancel_all_flow_tasks(&mut self) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("cancel_all_flow_tasks preflight")?;
        let tracked_run_ids = self.run_cancel_tokens.keys().cloned().collect::<Vec<_>>();
        for run_id in tracked_run_ids {
            let Some((token, flow_id)) = self
                .run_cancel_tokens
                .get(&run_id)
                .map(|(token, flow_id)| (token.clone(), flow_id.clone()))
            else {
                continue;
            };

            token.cancel();
            if let Some(handle) = self.run_tasks.remove(&run_id) {
                handle.abort();
            }
            self.flow_streams.lock().await.remove(&run_id);

            self.cancel_unfinished_steps_in_actor(&run_id).await?;
            self.terminalize_canceled_in_actor(
                run_id.clone(),
                flow_id.clone(),
                "cancel_all_flow_terminalize_canceled",
            )
            .await?;

            // CompleteFlow / FinishRun accept `active_run_count == 0` as
            // a legitimate terminal convergence (see
            // `CompleteFlowRunningZero` and `FinishRunRunningZero` in
            // the mob_machine DSL). The natural `FlowFinished` cleanup
            // races with this destroy-driven cancel; both paths drive
            // the authority toward the same terminal state.
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::CompleteFlow, "cancel_all_flow")?;
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::FinishRun, "cancel_all_flow")?;

            let _ = self.run_cancel_tokens.remove(&run_id);
        }
        self.ensure_flow_tracker_alignment("cancel_all_flow_tasks completion")
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Compensate a failed spawn wiring path to avoid partial state.
    async fn rollback_failed_spawn(
        &mut self,
        agent_identity: &MeerkatId,
        profile_name: &ProfileName,
        member_ref: &MemberRef,
        successful_wiring_targets: &[MeerkatId],
        planned_wiring_targets: &[MeerkatId],
    ) -> Result<(), MobError> {
        let spawned_entry = {
            let roster = self.roster.read().await;
            roster.get(agent_identity).cloned()
        };
        let retire_event_already_present =
            self.retire_event_exists(agent_identity, member_ref).await?;
        if spawned_entry.is_none() && !retire_event_already_present {
            return Err(MobError::WiringError(format!(
                "spawn rollback requires roster entry for '{agent_identity}' before retiring durable member state"
            )));
        }
        let mut wired_peers = successful_wiring_targets.to_vec();
        wired_peers.sort();
        wired_peers.dedup();

        let mut cleanup_peers = wired_peers.clone();
        for peer_id in planned_wiring_targets {
            if peer_id != agent_identity && !cleanup_peers.contains(peer_id) {
                cleanup_peers.push(peer_id.clone());
            }
        }
        let mut cleanup_handoffs = BTreeMap::new();
        if spawned_entry.is_some() {
            for peer_meerkat_id in &cleanup_peers {
                let peer_entry = {
                    let roster = self.roster.read().await;
                    roster.get(peer_meerkat_id).cloned()
                };
                let Some(peer_entry) = peer_entry else {
                    continue;
                };
                let cleanup_edge = mob_dsl::WiringEdge::new(
                    mob_dsl::AgentIdentity::from_domain(agent_identity),
                    mob_dsl::AgentIdentity::from_domain(&peer_entry.agent_identity),
                );
                match self.authorize_member_trust_cleanup(
                    &cleanup_edge,
                    "spawn_rollback_trust_cleanup_authority",
                ) {
                    Ok(handoff) => {
                        let retry_handoff = self
                            .authorize_member_trust_cleanup(
                                &cleanup_edge,
                                "spawn_rollback_trust_cleanup_retry_authority",
                            )
                            .ok();
                        cleanup_handoffs
                            .insert(peer_entry.agent_identity.clone(), (handoff, retry_handoff));
                    }
                    Err(error) => {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            peer = %peer_entry.agent_identity,
                            %error,
                            "spawn rollback trust cleanup skipped without generated unwiring authority"
                        );
                    }
                }
            }
        }
        let spawned_comms = self.provisioner_comms(member_ref).await;

        let mut rollback = LifecycleRollback::new("spawn rollback");

        if !wired_peers.is_empty() {
            let spawned_entry = spawned_entry.as_ref().ok_or_else(|| {
                MobError::WiringError(format!(
                    "spawn rollback requires roster entry for '{agent_identity}'"
                ))
            })?;
            let spawned_sender = self
                .sender_runtime_for_entry(spawned_entry)
                .await
                .ok_or_else(|| {
                    MobError::WiringError(format!(
                        "spawn rollback requires sender runtime for '{agent_identity}'"
                    ))
                })?;
            let spawned_spec = match self
                .resolve_wiring_endpoint(spawned_entry, "spawn rollback spawned member")
                .await?
            {
                WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
            };
            let spawned_peer_description = self
                .definition
                .resolve_profile(&spawned_entry.role, self.realm_profile_store.as_ref())
                .await
                .map(|p| p.peer_description)
                .unwrap_or_default();
            for peer_meerkat_id in &wired_peers {
                let peer_spec = {
                    let roster = self.roster.read().await;
                    let peer_entry = roster.get(peer_meerkat_id).cloned().ok_or_else(|| {
                        MobError::WiringError(format!(
                            "spawn rollback requires roster entry for wired peer '{peer_meerkat_id}'"
                        ))
                    })?;
                    drop(roster);
                    match self
                        .resolve_wiring_endpoint(&peer_entry, "spawn rollback")
                        .await?
                    {
                        WiringEndpoint::Local { spec, .. }
                        | WiringEndpoint::PeerOnly { spec, .. } => spec,
                    }
                };
                if let Err(error) = self
                    .notify_peer_retired(
                        &peer_spec,
                        agent_identity,
                        spawned_entry,
                        &spawned_spec,
                        &spawned_sender,
                    )
                    .await
                {
                    return Err(rollback.fail(error).await);
                }
                rollback.defer(
                    format!(
                        "compensating mob.peer_added '{agent_identity}' -> '{peer_meerkat_id}'"
                    ),
                    {
                        let spawned_sender = spawned_sender.clone();
                        let peer_spec = peer_spec.clone();
                        let agent_identity = agent_identity.clone();
                        let role = spawned_entry.role.clone();
                        let peer_description = spawned_peer_description.clone();
                        let spawned_spec = spawned_spec.clone();
                        move || async move {
                            let peer_route = PeerRoute::with_display_name(
                                peer_spec.peer_id,
                                peer_spec.name.clone(),
                            );
                            let cmd = CommsCommand::PeerLifecycle {
                                to: peer_route,
                                kind: PeerLifecycleKind::PeerAdded,
                                params: serde_json::json!({
                                    "peer": agent_identity.as_str(),
                                    "role": role.as_str(),
                                    "description": peer_description,
                                    "peer_name": spawned_spec.name,
                                    "peer_id": spawned_spec.peer_id,
                                    "address": spawned_spec.address,
                                    "peer_spec": spawned_spec,
                                }),
                            };
                            spawned_sender.send(cmd).await?;
                            Ok(())
                        }
                    },
                );
            }
        }

        if let Some(spawned_entry) = spawned_entry.as_ref()
            && let Ok(spawned_endpoint) = self
                .resolve_wiring_endpoint(spawned_entry, "spawn rollback trust cleanup spawned")
                .await
        {
            let (spawned_spec, spawned_comms, spawned_binding) = match spawned_endpoint {
                WiringEndpoint::Local { comms, spec, .. } => (spec, Some(comms), None),
                WiringEndpoint::PeerOnly { spec, binding } => (spec, None, Some(binding)),
            };
            for peer_meerkat_id in &cleanup_peers {
                let peer_entry = {
                    let roster = self.roster.read().await;
                    roster.get(peer_meerkat_id).cloned()
                };
                let Some(peer_entry) = peer_entry else {
                    continue;
                };
                let Ok(peer_endpoint) = self
                    .resolve_wiring_endpoint(&peer_entry, "spawn rollback trust cleanup peer")
                    .await
                else {
                    continue;
                };
                let (peer_spec, peer_comms, peer_binding) = match peer_endpoint {
                    WiringEndpoint::Local { comms, spec, .. } => (spec, Some(comms), None),
                    WiringEndpoint::PeerOnly { spec, binding } => (spec, None, Some(binding)),
                };
                let cleanup_handoff = cleanup_handoffs.get(&peer_entry.agent_identity);
                if let Some(spawned_comms) = spawned_comms.as_ref() {
                    let peer_key = Self::trusted_peer_removal_key(&peer_spec);
                    if let Some((handoff, retry_handoff)) = cleanup_handoff.as_ref() {
                        let authority =
                            handoff.unwiring_authority_for(&peer_entry.agent_identity, &peer_key);
                        if let Ok(authority) = authority {
                            let removed = self
                                .apply_trusted_peer_remove(
                                    spawned_comms.as_ref(),
                                    peer_key.clone(),
                                    authority,
                                )
                                .await;
                            if removed.is_err()
                                && let Some(retry_handoff) = retry_handoff.as_ref()
                                && let Ok(retry_authority) = retry_handoff
                                    .unwiring_authority_for(&peer_entry.agent_identity, &peer_key)
                            {
                                let _ = self
                                    .apply_trusted_peer_remove(
                                        spawned_comms.as_ref(),
                                        peer_key.clone(),
                                        retry_authority,
                                    )
                                    .await;
                            }
                        }
                    }
                }
                if let Some(peer_comms) = peer_comms {
                    let spawned_key = Self::trusted_peer_removal_key(&spawned_spec);
                    if let Some((handoff, retry_handoff)) = cleanup_handoff.as_ref() {
                        let authority =
                            handoff.unwiring_authority_for(agent_identity, &spawned_key);
                        if let Ok(authority) = authority {
                            let removed = self
                                .apply_trusted_peer_remove(
                                    peer_comms.as_ref(),
                                    spawned_key.clone(),
                                    authority,
                                )
                                .await;
                            if removed.is_err()
                                && let Some(retry_handoff) = retry_handoff.as_ref()
                                && let Ok(retry_authority) = retry_handoff
                                    .unwiring_authority_for(agent_identity, &spawned_key)
                            {
                                let _ = self
                                    .apply_trusted_peer_remove(
                                        peer_comms.as_ref(),
                                        spawned_key.clone(),
                                        retry_authority,
                                    )
                                    .await;
                            }
                        }
                    }
                }
                if spawned_binding.is_some() || peer_binding.is_some() {
                    if let Err(error) = self.cleanup_member_machine_wiring_edge(
                        agent_identity,
                        &peer_entry.agent_identity,
                        "spawn_rollback_peer_only_machine_wiring_cleanup",
                    ) {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            peer = %peer_entry.agent_identity,
                            %error,
                            "spawn rollback could not clean generated peer-only wiring graph"
                        );
                        continue;
                    }
                }
                if let Some(spawned_binding) = spawned_binding.as_ref()
                    && let Err(error) = self
                        .unwire_peer_only_recipient(
                            &spawned_spec,
                            Some(spawned_binding),
                            &peer_spec,
                            std::time::Duration::from_secs(2),
                        )
                        .await
                {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        peer = %peer_entry.agent_identity,
                        %error,
                        "spawn rollback failed to unwire spawned peer-only trust"
                    );
                }
                if let Some(peer_binding) = peer_binding.as_ref()
                    && let Err(error) = self
                        .unwire_peer_only_recipient(
                            &peer_spec,
                            Some(peer_binding),
                            &spawned_spec,
                            std::time::Duration::from_secs(2),
                        )
                        .await
                {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        peer = %peer_entry.agent_identity,
                        %error,
                        "spawn rollback failed to unwire peer-only trust for spawned member"
                    );
                }
            }
        }

        if !cleanup_peers.is_empty() {
            let rollback_inputs = cleanup_peers
                .iter()
                .filter(|peer_id| *peer_id != agent_identity)
                .map(|peer_id| mob_dsl::MobMachineInput::UnwireMembers {
                    edge: mob_dsl::WiringEdge::new(
                        mob_dsl::AgentIdentity::from_domain(agent_identity),
                        mob_dsl::AgentIdentity::from_domain(peer_id),
                    ),
                })
                .collect::<Vec<_>>();
            if !rollback_inputs.is_empty() {
                match self.prepare_dsl_inputs(&rollback_inputs, "spawn_rollback_wiring_cleanup") {
                    Ok(prepared) => {
                        if let Err(error) = self.commit_prepared_dsl_input(prepared) {
                            tracing::warn!(
                                mob_id = %self.definition.id,
                                %error,
                                "spawn rollback could not commit generated wiring graph cleanup"
                            );
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            %error,
                            "spawn rollback could not clean generated wiring graph"
                        );
                    }
                }
            }
        }

        if let Some(entry) = spawned_entry.as_ref() {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
            let releasing = member_ref
                .bridge_session_id()
                .map(mob_dsl::SessionId::from_domain);
            let session_id_for_route = releasing.clone();
            let prepared_retire = match self.prepare_dsl_input_transition(
                mob_dsl::MobMachineInput::Retire {
                    mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                    agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
                    agent_identity: dsl_identity.clone(),
                    generation: mob_dsl::Generation::from_domain(entry.generation),
                    releasing,
                    session_id: session_id_for_route.clone(),
                },
                "rollback_failed_spawn_mark_retiring",
            ) {
                Ok(prepared_retire) => prepared_retire,
                Err(error) => {
                    tracing::warn!(
                        agent_identity = %agent_identity,
                        %error,
                        "spawn rollback could not mark runtime retired in DSL"
                    );
                    return Err(rollback.fail(error).await);
                }
            };
            if let Err(error) = Self::require_member_lifecycle_journal_effect(
                &prepared_retire.transition,
                mob_dsl::MobLifecycleJournalKind::MemberRetired,
                &entry.agent_identity,
                &entry.agent_runtime_id,
                None,
                entry.generation,
                session_id_for_route,
                "rollback_failed_spawn_mark_retiring",
            ) {
                return Err(rollback.fail(error).await);
            }
            if !retire_event_already_present
                && let Err(error) = self.append_retire_event_for_entry(entry).await
            {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    %error,
                    "spawn rollback could not append durable retired event"
                );
                return Err(rollback.fail(error).await);
            }
            self.commit_prepared_dsl_transition(prepared_retire)?;
            if let Some(session_id) = member_ref.bridge_session_id() {
                self.discard_pending_routed_effects_for_session(session_id);
            }
        }

        // Reuse disposal pipeline methods for session archive + roster removal.
        let rollback_ctx = DisposalContext {
            agent_identity: agent_identity.clone(),
            entry: spawned_entry.clone().unwrap_or_else(|| {
                let identity = AgentIdentity::from(agent_identity.as_str());
                RosterEntry {
                    agent_identity: identity.clone(),
                    generation: crate::ids::Generation::INITIAL,
                    fence_token: crate::ids::FenceToken::new(0),
                    agent_runtime_id: crate::ids::AgentRuntimeId::initial(identity),
                    role: profile_name.clone(),
                    member_ref: member_ref.clone(),
                    runtime_mode: crate::MobRuntimeMode::TurnDriven,
                    peer_id: spawned_comms.as_ref().and_then(|c| c.peer_id()),
                    transport_public_key: spawned_comms.as_ref().and_then(|c| c.public_key()),
                    state: crate::roster::MemberState::Active,
                    wired_to: std::collections::BTreeSet::new(),
                    external_peer_specs: std::collections::BTreeMap::new(),
                    labels: std::collections::BTreeMap::new(),
                    kickoff: None,
                    effective_profile_override: None,
                }
            }),
            retiring_key: spawned_comms.as_ref().and_then(|c| c.public_key()),
            retiring_comms: None,
            retiring_spec: None,
            machine_wired_peer_identities: BTreeSet::new(),
            trust_unwire_authority_by_peer: BTreeMap::new(),
        };
        if let Err(error) = self.dispose_archive_session(&rollback_ctx).await {
            return Err(rollback.fail(error).await);
        }

        self.dispose_remove_from_roster(&rollback_ctx).await;
        self.delete_external_binding_overlay_for_member(
            &rollback_ctx.entry.agent_identity,
            rollback_ctx.entry.generation,
        )
        .await?;

        Ok(())
    }

    /// Resolve profile-declared rust tool bundles to a dispatcher.
    fn external_tools_for_profile(
        &self,
        profile: &crate::profile::Profile,
        per_spawn_external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
        let default_tools = self
            .default_external_tools_provider
            .as_ref()
            .and_then(|p| p());
        compose_external_tools_for_profile(
            profile,
            &self.tool_bundles,
            self.mob_handle_for_tools(),
            default_tools,
            per_spawn_external_tools,
            None,
        )
    }

    async fn retire_event_exists(
        &self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<bool, MobError> {
        let key = Self::retire_event_key(agent_identity, member_ref);
        let index = self.retired_event_index.read().await;
        Ok(index.contains(&key))
    }

    async fn append_retire_event_for_entry(&self, entry: &RosterEntry) -> Result<(), MobError> {
        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberRetired {
                    agent_identity: entry.agent_identity.clone(),
                    generation: entry.generation,
                    role: entry.role.clone(),
                },
            })
            .await?;
        let key = Self::retire_event_key(&entry.agent_identity, &entry.member_ref);
        self.retired_event_index.write().await.insert(key);
        Ok(())
    }

    /// Get the comms runtime for a session, if available.
    async fn provisioner_comms(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.provisioner.comms_runtime(member_ref).await
    }

    fn trusted_peer_descriptor_from_machine_endpoint(
        endpoint: &mob_dsl::ExternalPeerEndpoint,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        TrustedPeerDescriptor::unsigned_with_pubkey(
            endpoint.name.0.clone(),
            &endpoint.peer_id.0,
            endpoint.signing_key.0,
            &endpoint.address.0,
        )
        .map_err(|error| {
            MobError::WiringError(format!(
                "MobMachine external peer edge has invalid descriptor '{}': {error}",
                endpoint.name.0
            ))
        })
    }

    fn machine_restore_wiring_plan(
        &self,
        agent_identity: &MeerkatId,
    ) -> Result<RestoreWiringPlan, MobError> {
        let local =
            mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(agent_identity.as_str()));
        let mut local_peers = Vec::new();
        for edge in &self.dsl_authority.state().wiring_edges {
            let peer = if edge.a == local {
                Some(&edge.b)
            } else if edge.b == local {
                Some(&edge.a)
            } else {
                None
            };
            if let Some(peer) = peer {
                local_peers.push(MeerkatId::from(peer.0.as_str()));
            }
        }
        local_peers.sort();
        local_peers.dedup();

        let mut external_peers = Vec::new();
        for edge in self
            .dsl_authority
            .state()
            .external_peer_edges_by_key
            .values()
        {
            if edge.local == local {
                external_peers.push(Self::trusted_peer_descriptor_from_machine_endpoint(
                    &edge.endpoint,
                )?);
            }
        }
        external_peers.sort_by(|a, b| {
            a.name
                .as_str()
                .cmp(b.name.as_str())
                .then_with(|| a.peer_id.to_string().cmp(&b.peer_id.to_string()))
        });
        external_peers.dedup_by(|a, b| a.name == b.name && a.peer_id == b.peer_id);

        Ok(RestoreWiringPlan {
            local_peers,
            external_peers,
        })
    }

    async fn sender_runtime_for_entry(
        &self,
        entry: &RosterEntry,
    ) -> Option<Arc<dyn CoreCommsRuntime>> {
        if let Some(comms) = self.provisioner_comms(&entry.member_ref).await {
            return Some(comms);
        }
        if matches!(
            entry.member_ref,
            MemberRef::BackendPeer {
                session_id: None,
                ..
            }
        ) {
            return Some(self.supervisor_bridge.runtime_core().await);
        }
        None
    }

    /// Generate the comms name for a roster entry.
    fn comms_name_for(&self, entry: &RosterEntry) -> String {
        format!(
            "{}/{}/{}",
            self.definition.id, entry.role, entry.agent_identity
        )
    }

    async fn resolve_wiring_endpoint(
        &self,
        entry: &RosterEntry,
        context: &'static str,
    ) -> Result<WiringEndpoint, MobError> {
        let comms_name = self.comms_name_for(entry);
        let member_ref = self.machine_member_ref_for_behavior(entry, context)?;
        if let Some(comms) = self.provisioner_comms(&member_ref).await {
            let public_key = comms.public_key().ok_or_else(|| {
                MobError::WiringError(format!(
                    "{context} requires public key for '{}'",
                    entry.agent_identity
                ))
            })?;
            let spec = self
                .provisioner
                .trusted_peer_spec(&member_ref, &comms_name, &public_key)
                .await?;
            return Ok(WiringEndpoint::Local {
                entry: Box::new(entry.clone()),
                comms,
                spec,
                comms_name,
            });
        }

        match &member_ref {
            MemberRef::BackendPeer { .. } => {
                let binding =
                    Self::runtime_binding_for_member_ref(&member_ref).ok_or_else(|| {
                        MobError::WiringError(format!(
                            "{context} requires external runtime binding for '{}'",
                            entry.agent_identity
                        ))
                    })?;
                let spec = Self::peer_only_spec_for_binding(&binding, context)?;
                Ok(WiringEndpoint::PeerOnly { spec, binding })
            }
            MemberRef::Session { .. } => Err(MobError::WiringError(format!(
                "{context} requires comms runtime for '{}'",
                entry.agent_identity
            ))),
        }
    }

    /// Notify a peer that a new peer was added.
    ///
    /// Sends a `PeerRequest` with intent `mob.peer_added` FROM `sender_comms`
    /// TO the peer identified by `recipient_comms_name`. The params contain
    /// the new peer's identity and role.
    ///
    /// REQ-MOB-010/011: Notification is required for successful wiring.
    async fn notify_peer_added(
        &self,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
        recipient_spec: &TrustedPeerDescriptor,
        new_peer_id: &MeerkatId,
        new_peer_entry: &RosterEntry,
    ) -> Result<(), MobError> {
        let peer_description = self
            .definition
            .resolve_profile(&new_peer_entry.role, self.realm_profile_store.as_ref())
            .await
            .map(|p| p.peer_description)
            .unwrap_or_default();

        let new_peer_spec = match self
            .resolve_wiring_endpoint(new_peer_entry, "notify_peer_added")
            .await?
        {
            WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
        };
        let peer_route =
            PeerRoute::with_display_name(recipient_spec.peer_id, recipient_spec.name.clone());

        let cmd = CommsCommand::PeerLifecycle {
            to: peer_route,
            kind: PeerLifecycleKind::PeerAdded,
            params: serde_json::json!({
                "peer": new_peer_id.as_str(),
                "role": new_peer_entry.role.as_str(),
                "description": peer_description,
                "peer_name": new_peer_spec.name,
                "peer_id": new_peer_spec.peer_id,
                "address": new_peer_spec.address,
                "peer_spec": new_peer_spec,
            }),
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    async fn notify_peer_event(
        &self,
        intent: &'static str,
        recipient_spec: &TrustedPeerDescriptor,
        other_peer_id: &MeerkatId,
        other_peer_entry: &RosterEntry,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        let other_peer_spec = match self
            .resolve_wiring_endpoint(other_peer_entry, "notify_peer_event")
            .await?
        {
            WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
        };
        self.notify_peer_event_with_spec(
            intent,
            recipient_spec,
            other_peer_id,
            other_peer_entry,
            &other_peer_spec,
            sender_comms,
        )
        .await
    }

    async fn notify_peer_event_with_spec(
        &self,
        intent: &'static str,
        recipient_spec: &TrustedPeerDescriptor,
        other_peer_id: &MeerkatId,
        other_peer_entry: &RosterEntry,
        other_peer_spec: &TrustedPeerDescriptor,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        let peer_route =
            PeerRoute::with_display_name(recipient_spec.peer_id, recipient_spec.name.clone());

        let params = serde_json::json!({
            "peer": other_peer_id.as_str(),
            "role": other_peer_entry.role.as_str(),
            "peer_name": other_peer_spec.name.clone(),
            "peer_id": other_peer_spec.peer_id,
            "address": other_peer_spec.address.clone(),
            "peer_spec": other_peer_spec.clone(),
        });

        let cmd = match intent {
            "mob.peer_retired" => CommsCommand::PeerLifecycle {
                to: peer_route,
                kind: PeerLifecycleKind::PeerRetired,
                params,
            },
            "mob.peer_unwired" => CommsCommand::PeerLifecycle {
                to: peer_route,
                kind: PeerLifecycleKind::PeerUnwired,
                params,
            },
            _ => CommsCommand::PeerRequest {
                to: peer_route,
                intent: intent.to_string(),
                params,
                blocks: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                stream: meerkat_core::comms::InputStreamMode::None,
            },
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    async fn notify_kickoff_event(
        &self,
        agent_identity: &MeerkatId,
        intent: &'static str,
    ) -> Result<(), MobError> {
        let (entry, wired_peers) = {
            let machine_peer_identities = self
                .machine_wired_peer_identities_for(&AgentIdentity::from(agent_identity.as_str()));
            let roster = self.roster.read().await;
            let Some(entry) = roster.get(agent_identity).cloned() else {
                return Ok(());
            };
            let wired_peers: Vec<MeerkatId> = machine_peer_identities
                .iter()
                .filter_map(|id| roster.get_by_identity(id).map(|e| e.agent_identity.clone()))
                .collect();
            (entry, wired_peers)
        };
        let sender_comms = self.provisioner_comms(&entry.member_ref).await;
        let effects = MobRuntimeBridgeAuthority::plan_lifecycle_notice(
            sender_comms.is_some()
                || matches!(
                    entry.member_ref,
                    MemberRef::BackendPeer {
                        session_id: None,
                        ..
                    }
                ),
            &wired_peers,
            intent,
        );

        let Some(sender_comms) = self.sender_runtime_for_entry(&entry).await else {
            return Ok(());
        };

        for effect in effects {
            let MobRuntimeBridgeEffect::DeliverLifecycleNotice { peer_id, intent } = effect;
            let recipient_entry = {
                let roster = self.roster.read().await;
                roster.get(&peer_id).cloned()
            };
            let Some(recipient_entry) = recipient_entry else {
                continue;
            };
            let recipient_spec = match self
                .resolve_wiring_endpoint(&recipient_entry, "notify_kickoff_event")
                .await?
            {
                WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
            };
            self.notify_peer_event(
                intent,
                &recipient_spec,
                agent_identity,
                &entry,
                &sender_comms,
            )
            .await?;
        }
        Ok(())
    }

    /// Notify a peer that another peer was retired from the mob.
    async fn notify_peer_retired(
        &self,
        recipient_spec: &TrustedPeerDescriptor,
        retired_id: &MeerkatId,
        retired_entry: &RosterEntry,
        retired_spec: &TrustedPeerDescriptor,
        retiring_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        self.notify_peer_event_with_spec(
            "mob.peer_retired",
            recipient_spec,
            retired_id,
            retired_entry,
            retired_spec,
            retiring_comms,
        )
        .await
    }

    /// Notify a peer that another peer was unwired (trust link removed).
    async fn notify_peer_unwired(
        &self,
        recipient_spec: &TrustedPeerDescriptor,
        unwired_id: &MeerkatId,
        unwired_entry: &RosterEntry,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        self.notify_peer_event(
            "mob.peer_unwired",
            recipient_spec,
            unwired_id,
            unwired_entry,
            sender_comms,
        )
        .await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod runtime_observation_tests {
    use super::{foreign_runtime_observation, mob_dsl};

    #[test]
    fn foreign_runtime_observations_are_identified_for_log_downgrade() {
        let authority = mob_dsl::MobMachineAuthority::new();
        let signal = mob_dsl::MobMachineSignal::ObserveRuntimeReady {
            agent_runtime_id: mob_dsl::AgentRuntimeId("rt:session:parent".to_string()),
            fence_token: mob_dsl::FenceToken(0),
        };

        assert!(foreign_runtime_observation(authority.state(), &signal).is_some());
    }

    #[test]
    fn live_member_runtime_observations_remain_machine_owned() {
        let mut authority = mob_dsl::MobMachineAuthority::new();
        let identity = mob_dsl::AgentIdentity("member".to_string());
        let runtime_id = mob_dsl::AgentRuntimeId("member:0".to_string());
        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::AuthorizeSpawnProfile {
                agent_identity: identity.clone(),
                profile_name: "test".to_string(),
                model: "test-model".to_string(),
                profile_material_digest: "test-profile-digest".to_string(),
                tool_config_digest: "test-tool-config-digest".to_string(),
                skills_digest: "test-skills-digest".to_string(),
                provider_params_digest: None,
                output_schema_digest: None,
                external_addressable: true,
            },
        )
        .expect("AuthorizeSpawnProfile should seed live runtime ownership");
        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::Spawn {
                agent_identity: identity,
                agent_runtime_id: runtime_id.clone(),
                fence_token: mob_dsl::FenceToken(1),
                generation: mob_dsl::Generation(0),
                profile_material_digest: "test-profile-digest".to_string(),
                external_addressable: true,
                runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::AutonomousHost,
                bridge_session_id: Some(mob_dsl::SessionId("member-session".to_string())),
                replacing: None,
            },
        )
        .expect("Spawn should seed live runtime ownership");
        let signal = mob_dsl::MobMachineSignal::ObserveRuntimeReady {
            agent_runtime_id: runtime_id,
            fence_token: mob_dsl::FenceToken(1),
        };

        assert!(foreign_runtime_observation(authority.state(), &signal).is_none());
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod bridge_rejection_tests {
    use super::{ExpectedRevokeCleanupFailure, MobActor};
    use crate::MobError;
    use crate::runtime::bridge_protocol::{
        BridgeRejectionCause, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        decode_legacy_v1_raw_string_rejection,
    };
    use meerkat_core::comms::{AdmissionDropReason, SendError};
    use serde_json::json;

    fn known_bridge_rejection_causes() -> &'static [(BridgeRejectionCause, &'static str)] {
        &[
            (BridgeRejectionCause::NotBound, "not_bound"),
            (BridgeRejectionCause::StaleSupervisor, "stale_supervisor"),
            (BridgeRejectionCause::SenderMismatch, "sender_mismatch"),
            (BridgeRejectionCause::AlreadyBound, "already_bound"),
            (
                BridgeRejectionCause::InvalidBootstrapToken,
                "invalid_bootstrap_token",
            ),
            (
                BridgeRejectionCause::UnsupportedProtocolVersion,
                "unsupported_protocol_version",
            ),
            (
                BridgeRejectionCause::InvalidSupervisorSpec,
                "invalid_supervisor_spec",
            ),
            (BridgeRejectionCause::InvalidPeerSpec, "invalid_peer_spec"),
            (BridgeRejectionCause::AddressMismatch, "address_mismatch"),
            (BridgeRejectionCause::Unsupported, "unsupported"),
            (BridgeRejectionCause::Internal, "internal"),
        ]
    }

    #[test]
    fn actor_decodes_typed_protocol_v2_bridge_rejection() {
        let value = json!({
            "result": "rejected",
            "cause": "stale_supervisor",
            "reason": "epoch too low",
        });

        let rejection =
            MobActor::bridge_rejection_reply(SUPERVISOR_BRIDGE_PROTOCOL_VERSION, &value)
                .expect("typed rejection should decode");

        assert_eq!(
            rejection.typed_cause(),
            Some(BridgeRejectionCause::StaleSupervisor)
        );
        assert_eq!(rejection.reason(), "epoch too low");
    }

    #[test]
    fn actor_does_not_promote_raw_string_as_protocol_v2_rejection() {
        let value = json!("legacy rejection");

        assert!(
            MobActor::bridge_rejection_reply(SUPERVISOR_BRIDGE_PROTOCOL_VERSION, &value).is_none()
        );
        let legacy = decode_legacy_v1_raw_string_rejection(&value)
            .expect("legacy raw string should decode only through the explicit v1 helper");
        assert_eq!(legacy.typed_cause(), None);
        assert!(legacy.is_legacy_v1_raw_string());
    }

    #[test]
    fn actor_bridge_rejection_error_preserves_each_known_typed_cause() {
        for (cause, wire_name) in known_bridge_rejection_causes() {
            let reason = format!("typed bridge rejection: {wire_name}");
            let value = json!({
                "result": "rejected",
                "cause": wire_name,
                "reason": reason,
            });
            let rejection =
                MobActor::bridge_rejection_reply(SUPERVISOR_BRIDGE_PROTOCOL_VERSION, &value)
                    .expect("typed rejection should decode");

            let error = MobActor::bridge_rejection_error(rejection);

            match error {
                MobError::BridgeCommandRejected {
                    cause: actual,
                    reason: actual_reason,
                } => {
                    assert_eq!(actual, *cause);
                    assert_eq!(actual_reason, reason);
                }
                other => panic!("expected typed bridge rejection error, got {other:?}"),
            }
        }
    }

    #[test]
    fn actor_legacy_bridge_rejection_error_stays_untyped() {
        let value = json!("legacy rejection");
        let legacy = decode_legacy_v1_raw_string_rejection(&value)
            .expect("legacy raw string should decode only through the explicit v1 helper");

        let error = MobActor::bridge_rejection_error(legacy);

        assert!(matches!(
            error,
            MobError::WiringError(reason) if reason == "legacy rejection"
        ));
    }

    #[test]
    fn revoke_cleanup_expected_failures_are_typed() {
        let cases = [
            (
                MobError::CommsError(SendError::PeerNotFound("peer-1".to_string())),
                ExpectedRevokeCleanupFailure::CommsPeerNotFound,
            ),
            (
                MobError::CommsError(SendError::PeerOffline),
                ExpectedRevokeCleanupFailure::CommsPeerOffline,
            ),
            (
                MobError::CommsError(SendError::AdmissionDropped {
                    reason: AdmissionDropReason::UntrustedSender,
                }),
                ExpectedRevokeCleanupFailure::CommsAdmissionDropped {
                    reason: AdmissionDropReason::UntrustedSender,
                },
            ),
            (
                MobError::CommsError(SendError::AdmissionDropped {
                    reason: AdmissionDropReason::SessionClosed,
                }),
                ExpectedRevokeCleanupFailure::CommsAdmissionDropped {
                    reason: AdmissionDropReason::SessionClosed,
                },
            ),
            (
                MobError::BridgeCommandRejected {
                    cause: BridgeRejectionCause::NotBound,
                    reason: "no authorized supervisor registered".to_string(),
                },
                ExpectedRevokeCleanupFailure::BridgeRejected {
                    cause: BridgeRejectionCause::NotBound,
                },
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(
                MobActor::expected_revoke_cleanup_failure(&error),
                Some(expected)
            );
        }
    }

    #[test]
    fn revoke_cleanup_does_not_treat_error_text_as_a_semantic_cause() {
        let cases = [
            MobError::WiringError("peer offline".to_string()),
            MobError::Internal("no authorized supervisor registered".to_string()),
            MobError::CommsError(SendError::Validation("peer not found".to_string())),
            MobError::CommsError(SendError::AdmissionDropped {
                reason: AdmissionDropReason::InboxFull,
            }),
            MobError::BridgeCommandRejected {
                cause: BridgeRejectionCause::SenderMismatch,
                reason: "no authorized supervisor registered".to_string(),
            },
        ];

        for error in cases {
            assert_eq!(MobActor::expected_revoke_cleanup_failure(&error), None);
        }
    }

    #[test]
    fn revoke_cleanup_expected_failure_ratchet_rejects_string_parsing() {
        let source = include_str!("actor.rs");
        let start = source
            .find("fn expected_revoke_cleanup_failure")
            .expect("expected cleanup classifier exists");
        let end = source[start..]
            .find("\n    async fn destroy_remote_member_for_destroy")
            .expect("cleanup classifier precedes remote destroy cleanup");
        let body = &source[start..start + end];

        for disallowed in [
            "to_ascii_lowercase",
            "to_lowercase",
            "error.to_string()",
            ".contains(",
        ] {
            assert!(
                !body.contains(disallowed),
                "expected revoke cleanup classifier must not parse error text with {disallowed}"
            );
        }
    }
}
