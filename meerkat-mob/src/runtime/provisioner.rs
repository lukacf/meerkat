#[cfg(feature = "runtime-adapter")]
use super::bridge::MobBoundMemberRuntimeBridge;
#[cfg(feature = "runtime-adapter")]
use super::local_bridge::LocalMobRuntimeBridge;
use super::*;
use crate::RuntimeBinding;
use crate::definition::ExternalBackendConfig;
use crate::event::MemberRef;
use crate::runtime::handle::MemberSpawnReceipt;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use meerkat_core::PendingSystemContextAppend;
use meerkat_core::comms::{PeerAddress, PeerName, TrustedPeerDescriptor};
use meerkat_core::event_injector::SubscribableInjector;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle, CoreExecutorPostStopCleanupHandle, CoreExecutorPublicationHandle,
    CoreExecutorTurnFinalizationBoundaryHandle, CoreExecutorTurnFinalizationGuard,
};
#[cfg(feature = "runtime-adapter")]
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunApplyBoundary, RunPrimitive};
#[cfg(feature = "runtime-adapter")]
use meerkat_core::lifecycle::{InputId, RunId as CoreRunId};
use meerkat_core::ops::OperationId;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::ops_lifecycle::OperationStatus;
use meerkat_core::ops_lifecycle::{OperationSource, OpsLifecycleRegistry};
use meerkat_core::service::{CreateSessionRequest, SessionError, StartTurnRequest};
use meerkat_core::time_compat::Duration;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::time_compat::Instant;
use meerkat_core::types::SessionId;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::{TurnTerminalClassifier, TurnTerminalKind};
#[cfg(feature = "runtime-adapter")]
#[allow(unused_imports)]
use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
#[cfg(feature = "runtime-adapter")]
use meerkat_runtime::{
    EnsureRuntimeExecutorAttachment, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, MeerkatMachine, PendingRuntimeExecutorAttachment,
    PreparedAttachedSessionActorRecovery, PreparedSessionMaterialization, PromptInput,
    RuntimeExecutorAttachmentWitness, RuntimeSessionRegistrationWitness,
};
#[cfg(feature = "runtime-adapter")]
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::sync::Mutex as StdMutex;
#[cfg(feature = "runtime-adapter")]
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};

type TurnEventTx = tokio::sync::mpsc::Sender<meerkat_core::EventEnvelope<meerkat_core::AgentEvent>>;

#[cfg(not(target_arch = "wasm32"))]
type ArchiveDisposalFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<crate::machines::mob_machine::MemberSessionDisposal, SessionError>,
            > + Send
            + 'a,
    >,
>;

#[cfg(target_arch = "wasm32")]
type ArchiveDisposalFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<crate::machines::mob_machine::MemberSessionDisposal, SessionError>,
            > + 'a,
    >,
>;

#[cfg(feature = "runtime-adapter")]
const DEFERRED_TURN_EVENT_CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub(crate) struct PeerOnlyRebindObservation {
    pub observed_peer: TrustedPeerDescriptor,
    pub bootstrap_token: super::bridge_protocol::BridgeBootstrapToken,
    /// The raw typed wire rejection cause carried up to the machine-owning
    /// caller. The provisioner does NOT reduce this into a recoverable-vs-fatal
    /// conclusion — that semantic verdict is owned by MobMachine and decided by
    /// the caller (resume builder) via `classify_bridge_rejection_recovery`.
    pub rejection_cause: super::bridge_protocol::BridgeRejectionCause,
}

#[derive(Debug, Clone)]
pub(crate) struct PeerOnlyRebindAuthority {
    pub peer: TrustedPeerDescriptor,
    pub bootstrap_token: super::bridge_protocol::BridgeBootstrapToken,
}

#[derive(Debug, Clone)]
pub(crate) struct PeerOnlyTrustOverlay {
    handoff: super::bridge_protocol::BridgeMobPeerOverlayHandoff,
    peers: Vec<TrustedPeerDescriptor>,
}

impl PeerOnlyTrustOverlay {
    pub(crate) fn from_generated_mob_member_peer_overlay(
        obligation: &crate::generated::protocol_mob_member_peer_overlay::MobMemberPeerOverlayObligation,
    ) -> Result<Self, MobError> {
        crate::generated::protocol_mob_member_peer_overlay::validate_overlay_freshness(obligation)
            .map_err(MobError::WiringError)?;
        let mut peers = obligation
            .peer_overlay_endpoints()
            .iter()
            .map(trusted_peer_descriptor_from_generated_member_endpoint)
            .collect::<Result<Vec<_>, _>>()?;
        peers.sort_by(|a, b| {
            a.peer_id
                .to_string()
                .cmp(&b.peer_id.to_string())
                .then_with(|| a.name.as_str().cmp(b.name.as_str()))
                .then_with(|| a.address.to_string().cmp(&b.address.to_string()))
        });
        let mut seen_peer_ids = std::collections::BTreeSet::new();
        for peer in &peers {
            if !seen_peer_ids.insert(peer.peer_id.to_string()) {
                return Err(MobError::WiringError(format!(
                    "generated MobMachine peer overlay for '{}' contains duplicate peer '{}'",
                    obligation.agent_identity().0,
                    peer.peer_id
                )));
            }
        }
        let handoff = super::bridge_protocol::BridgeMobPeerOverlayHandoff {
            recipient_peer_id: obligation.peer_id().0.clone(),
            topology_epoch: obligation.epoch(),
            peer_specs: peers
                .iter()
                .cloned()
                .map(super::bridge_protocol::BridgePeerSpec::from)
                .collect(),
        };
        Ok(Self { handoff, peers })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    pub(crate) fn peers(&self) -> &[TrustedPeerDescriptor] {
        &self.peers
    }

    pub(crate) fn bridge_handoff(&self) -> super::bridge_protocol::BridgeMobPeerOverlayHandoff {
        self.handoff.clone()
    }
}

fn trusted_peer_descriptor_from_generated_member_endpoint(
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
            "generated MobMachine peer overlay carried invalid endpoint '{}': {error}",
            endpoint.name.0
        ))
    })
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PeerOnlyTrustReconcileReport {
    pub rebind_required: Option<PeerOnlyRebindObservation>,
}

#[derive(Debug, Clone)]
struct PeerOnlySupervisorAuthorization {
    peer: TrustedPeerDescriptor,
    rebind_required: Option<PeerOnlyRebindObservation>,
}

#[cfg(feature = "runtime-adapter")]
struct DeferredTurnEventDelivery {
    release_tx: oneshot::Sender<DeferredTurnEventOutcome>,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeferredTurnEventOutcome {
    Succeeded,
    CallbackPending,
    ExtractionFailed,
    Failed,
}

#[cfg(feature = "runtime-adapter")]
impl DeferredTurnEventDelivery {
    fn release(self, outcome: DeferredTurnEventOutcome) {
        let _ = self.release_tx.send(outcome);
    }
}

#[cfg(feature = "runtime-adapter")]
fn deferred_terminal_kind_matches_outcome(
    terminal_kind: TurnTerminalKind,
    outcome: DeferredTurnEventOutcome,
) -> bool {
    match terminal_kind {
        TurnTerminalKind::RunCompleted
        | TurnTerminalKind::ExtractionSucceeded
        | TurnTerminalKind::InteractionComplete => outcome == DeferredTurnEventOutcome::Succeeded,
        TurnTerminalKind::InteractionCallbackPending => {
            outcome == DeferredTurnEventOutcome::CallbackPending
        }
        TurnTerminalKind::ExtractionFailed => outcome == DeferredTurnEventOutcome::ExtractionFailed,
        TurnTerminalKind::RunFailed
        | TurnTerminalKind::InteractionFailed
        | TurnTerminalKind::ChannelClosed => outcome == DeferredTurnEventOutcome::Failed,
    }
}

/// Collapse the observer set for one batched runtime primitive into the single
/// event sender accepted by `StartTurnRequest` without losing request-local
/// streams. Every destination here is already the output of
/// `defer_turn_events_until_machine_completion`, so terminal commit gating
/// remains independent for each contributing input.
///
/// Sends for one envelope are polled concurrently so a healthy destination is
/// not ordered behind a slower sibling. The next envelope waits for every live
/// destination, preserving the original bounded backpressure contract and a
/// complete, ordered stream. Only closed destinations are removed.
#[cfg(feature = "runtime-adapter")]
fn fan_out_turn_event_txs(event_txs: Vec<TurnEventTx>) -> Option<TurnEventTx> {
    fan_out_turn_event_txs_with_capacity(event_txs, DEFERRED_TURN_EVENT_CHANNEL_CAPACITY)
}

#[cfg(feature = "runtime-adapter")]
fn fan_out_turn_event_txs_with_capacity(
    mut event_txs: Vec<TurnEventTx>,
    channel_capacity: usize,
) -> Option<TurnEventTx> {
    match event_txs.len() {
        0 => None,
        1 => event_txs.pop(),
        _ => {
            let (fanout_tx, mut fanout_rx) = mpsc::channel::<
                meerkat_core::EventEnvelope<meerkat_core::AgentEvent>,
            >(channel_capacity);
            tokio::spawn(async move {
                while let Some(event) = fanout_rx.recv().await {
                    event_txs = futures::future::join_all(event_txs.drain(..).map(|event_tx| {
                        let event = event.clone();
                        async move { event_tx.send(event).await.map(|()| event_tx) }
                    }))
                    .await
                    .into_iter()
                    .filter_map(Result::ok)
                    .collect();
                    if event_txs.is_empty() {
                        break;
                    }
                }
            });
            Some(fanout_tx)
        }
    }
}

#[cfg(feature = "runtime-adapter")]
fn defer_turn_events_until_machine_completion(
    _session_id: &SessionId,
    event_tx: Option<TurnEventTx>,
) -> (Option<TurnEventTx>, Option<DeferredTurnEventDelivery>) {
    let Some(event_tx) = event_tx else {
        return (None, None);
    };

    let (deferred_tx, mut deferred_rx) = mpsc::channel::<
        meerkat_core::EventEnvelope<meerkat_core::AgentEvent>,
    >(DEFERRED_TURN_EVENT_CHANNEL_CAPACITY);
    let (release_tx, release_rx) = oneshot::channel();
    tokio::spawn(async move {
        let mut release_rx = Box::pin(release_rx);
        let mut terminal_classifier = TurnTerminalClassifier::new();
        let mut buffered_terminal = Vec::new();
        let mut stream_closed = false;
        let mut released = None;

        loop {
            tokio::select! {
                event = deferred_rx.recv(), if !stream_closed => {
                    match event {
                        Some(event) => {
                            let terminal_kind = terminal_classifier
                                .observe(&event.payload)
                                .map(|terminal| terminal.kind);
                            match (terminal_kind, released) {
                                (Some(terminal_kind), None) => {
                                    buffered_terminal.push((event, terminal_kind));
                                }
                                (Some(terminal_kind), Some(outcome)) => {
                                    if deferred_terminal_kind_matches_outcome(
                                        terminal_kind,
                                        outcome,
                                    ) && event_tx.send(event).await.is_err()
                                    {
                                        return;
                                    }
                                }
                                (None, _) => {
                                    if event_tx.send(event).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        None => stream_closed = true,
                    }
                }
                result = &mut release_rx, if released.is_none() => {
                    // A dropped release authority is failure-closed: never let a
                    // buffered success terminal escape without a matching
                    // machine completion result.
                    let outcome = result.unwrap_or(DeferredTurnEventOutcome::Failed);
                    released = Some(outcome);
                    for (event, terminal_kind) in buffered_terminal.drain(..) {
                        if deferred_terminal_kind_matches_outcome(terminal_kind, outcome)
                            && event_tx.send(event).await.is_err()
                        {
                            return;
                        }
                    }
                }
            }

            if stream_closed && released.is_some() {
                break;
            }
        }
        // No synthetic terminal event. The mob actor receives the
        // authoritative terminal signal from CompletionOutcome via
        // runtime_completion_to_mob_result(). Shell code must not
        // fabricate EventEnvelopes with invented sequence numbers
        // (dogma §3: shell owns mechanics, not meaning).
    });

    (
        Some(deferred_tx),
        Some(DeferredTurnEventDelivery { release_tx }),
    )
}

pub struct ProvisionMemberRequest {
    pub create_session: CreateSessionRequest,
    /// Whether provisioning created a new durable session or is temporarily
    /// attaching an existing one. Cancellation only quiesces the exact live
    /// incarnation; durable session lifecycle changes require an explicit
    /// owner operation.
    pub session_origin: ProvisionSessionOrigin,
    pub binding: RuntimeBinding,
    pub peer_name: String,
    pub(crate) owner_bridge_session_id: Option<SessionId>,
    pub ops_registry: Option<Arc<dyn OpsLifecycleRegistry>>,
    pub(crate) generated_self_owned_operation_owner: Option<SessionId>,
    /// Internal proof that MobMachine authorized rebuilding a durable member
    /// whose live session materialization is missing in this process. Only
    /// that delivery-time revival may publish the missing-owner observation
    /// and re-run same-session readmission before attaching its replacement.
    pub(crate) runtime_revival_intent: RuntimeRevivalIntent,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum RuntimeRevivalIntent {
    #[default]
    None,
    MissingLiveMaterialization,
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProvisionSessionOrigin {
    Fresh,
    ResumedDurable,
    RevivedRetired,
}

/// ADJ-3: the `MaterializeMember` bridge budget. Member builds include
/// session create + preflight; the send holds the bridge single-flight lock
/// for its whole duration (relaxation is phase 6). The ADJ-4 single resend
/// gives an effective 120s ceiling.
pub(crate) const MATERIALIZE_BRIDGE_TIMEOUT: Duration = Duration::from_secs(60);

/// DEC-R4: the `ReleaseMember` bridge budget (remote quiesce + drain +
/// commit-first archive; the BindHost 60s precedent).
pub(crate) const HOST_RELEASE_TIMEOUT: Duration = Duration::from_secs(60);

/// DEC-P6E-8: per-attempt `HardCancelMember` bridge budget. The receiver
/// withholds ACK until the exact expected run is unbound/terminal, while a
/// transient resend reuses the byte-identical operation/run tuple.
pub(crate) const HARD_CANCEL_BRIDGE_TIMEOUT: Duration = Duration::from_secs(5);

/// Exact tracked-input cancellation waits for the member host to durably
/// fence and, when necessary, quiesce a racing admission. Its transport
/// envelope must remain strictly larger than the host's 5s settle budget.
pub(crate) const TRACKED_INPUT_CANCEL_BRIDGE_TIMEOUT: Duration = Duration::from_secs(15);

/// DEC-P6F-4: directive-bearing delivery budget per attempt (the plain
/// peer-delivery 5s budget), with exactly ONE ADJ-4-class resend on
/// send-timeout / wire `Unavailable`.
pub(crate) const TURN_DIRECTIVE_DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Input to [`MobProvisioner::materialize_member`]. The PAYLOAD is fully
/// built by the ACTOR from the machine's `RequestMemberMaterialization`
/// effect (owner realization); the provisioner is transport.
pub struct MaterializeMemberRequest {
    /// Bound host peer (endpoint + pubkey from MobMachine host maps —
    /// machine-owned facts).
    pub host_peer: TrustedPeerDescriptor,
    pub payload: Box<super::bridge_protocol::BridgeMaterializePayload>,
    /// `render_member_comms_name` output for the member.
    pub peer_name: String,
    /// REQUIRED (not Option): §19.L5 invariant — placed members always bind
    /// a controlling-side ops owner; the machine guard
    /// `owner_bridge_present` already refused None.
    pub owner_bridge_session_id: SessionId,
    pub ops_registry: Arc<dyn OpsLifecycleRegistry>,
    /// Canonical operation id durably anchored in the Pending placed carrier
    /// before this transport is allowed to send.
    pub provision_operation_id: OperationId,
    /// Whether this dispatch is realizing a newly persisted Pending carrier
    /// or re-materializing an already Committed carrier.  The caller performs
    /// the corresponding exact lifecycle preflight before transport; this
    /// mode controls the post-ACK transition without rediscovering/minting an
    /// operation from the returned endpoint.
    pub operation_anchor: PlacedProvisionOperationAnchor,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacedProvisionOperationAnchor {
    /// Exact operation was registered in Provisioning after Pending became
    /// durable and before any bridge send.
    NewPending,
    /// Exact operation belongs to an existing Committed carrier and was
    /// verified Running before the revival bridge send.
    ExistingCommitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacedOperationRecoveryExpectation {
    /// Normal live/revival path: only exact Running is compatible.
    Active,
    /// Retirement recovery: exact Running or Retiring is compatible; a
    /// reconstructed/Provisioning row first becomes Running and the
    /// retirement retry owns the subsequent Retiring transition.
    Retiring,
}

/// Receipt from a successful materialization.
pub struct MaterializedSpawnReceipt {
    pub(crate) receipt: MemberSpawnReceipt,
}

/// Domain carrier of the transport-validated `MemberMaterialized` ack facts
/// (design §4.2 step 6). The digest/engine-version echoes are NOT verified
/// here — they are MACHINE guards at `CommitSpawnMembershipRemote`; the
/// provisioner validates only the transport-integrity facts it owns.
#[derive(Debug, Clone)]
pub(crate) struct MaterializedMemberAck {
    /// Canonical, identity-validated member peer descriptor.
    pub(crate) member_peer: TrustedPeerDescriptor,
    /// Remote session id — machine-bound only (DEC-P3-6): never stuffed
    /// into the shell `MemberRef`.
    pub(crate) session_id: SessionId,
    pub(crate) spec_digest_echo: String,
    pub(crate) engine_version: String,
    #[allow(dead_code)]
    pub(crate) launch_outcome: super::bridge_protocol::MaterializeLaunchOutcome,
    #[allow(dead_code)]
    pub(crate) resolved_auth_binding: Option<meerkat_core::AuthBindingRef>,
}

/// Input to [`MobProvisioner::release_host_member`].
#[derive(Clone)]
pub struct HostMemberReleaseRequest {
    pub mob_id: crate::MobId,
    pub agent_identity: String,
    pub generation: u64,
    pub fence_token: u64,
    /// Immutable authority snapshot captured before the detached release is
    /// spawned. The provisioner must never re-read the live supervisor.
    pub supervisor_authority: crate::store::SupervisorAuthorityRecord,
    pub supervisor: super::bridge_protocol::BridgePeerSpec,
    pub binding_generation: u64,
    /// Host route material — projection of MobMachine host maps, resolved
    /// by the actor (the provisioner has no machine state).
    pub host: TrustedPeerDescriptor,
}

/// Actor-minted authority and stable correlation for one placed plain turn.
/// Every field comes from the controlling MobMachine placement/incarnation;
/// the provisioner only realizes transport and never infers placement from a
/// `MemberRef` shape.
#[derive(Debug, Clone)]
pub(crate) struct PlacedTurnDeliveryContext {
    pub input_id: String,
    /// Caller-supplied transcript identity, when present. Admission keeps
    /// absence as `None` so the member runtime retains its normal minting
    /// semantics; completion supplies the exact sidecar/waiter correlation.
    pub transcript_interaction_id: Option<String>,
    pub expected_member: super::bridge_protocol::BridgeMemberIncarnation,
    /// Explicit durable host-terminal custody. Ingress-only placed turns omit
    /// it; autonomous kickoff and ordinary AwaitTurnCompletion both pair
    /// `Interaction` with matching machine custody. The exact volatile waiter
    /// (if still present) resolves only after durable machine convergence and
    /// before ACK eligibility.
    pub outcome_tracking: Option<super::bridge_protocol::BridgeOutcomeTracking>,
}

/// Exact attachment-local authority carried from a successful resume until
/// the surrounding mob roster transaction commits.
///
/// The type is public only because [`MobProvisioner`] is a public extension
/// seam; its witness is intentionally opaque and can be minted only by the
/// runtime-backed provisioner after validating the machine and sidecar pair.
#[doc(hidden)]
#[derive(Clone)]
pub struct ResumedMemberRollbackAuthority {
    #[cfg(feature = "runtime-adapter")]
    state: Option<Arc<RuntimeSessionState>>,
    #[cfg(not(feature = "runtime-adapter"))]
    _private: (),
}

impl std::fmt::Debug for ResumedMemberRollbackAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(feature = "runtime-adapter")]
        {
            formatter
                .debug_struct("ResumedMemberRollbackAuthority")
                .field("witness", &self.state.as_ref().map(|state| state.witness()))
                .finish()
        }
        #[cfg(not(feature = "runtime-adapter"))]
        formatter
            .debug_struct("ResumedMemberRollbackAuthority")
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "runtime-adapter")]
impl ResumedMemberRollbackAuthority {
    fn new(state: Arc<RuntimeSessionState>) -> Self {
        Self { state: Some(state) }
    }

    fn state(&self) -> Option<&Arc<RuntimeSessionState>> {
        self.state.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self { state: None }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobProvisioner: Send + Sync {
    /// Whether this exact member delivery path can apply a per-turn LLM
    /// identity at the runtime executor boundary. This is intentionally
    /// member-specific: an actor may host lifecycle adapters alongside direct,
    /// peer-only, or remotely hosted backends that cannot realize an override.
    fn supports_member_turn_llm_reconfigure(&self, _member_ref: &MemberRef) -> bool {
        false
    }

    async fn provision_member(
        &self,
        req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError>;
    async fn abort_member_provision(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError>;
    /// Capture the exact executor attachment that a later failed roster
    /// commit is allowed to roll back. Implementations must fail closed when
    /// they cannot prove a live, matching attachment; a SessionId alone is
    /// never rollback authority.
    async fn capture_resumed_member_rollback_authority(
        &self,
        member_ref: &MemberRef,
    ) -> Result<ResumedMemberRollbackAuthority, MobError>;
    /// Release a failed resume provision without archiving its durable
    /// session. The runtime is returned to durable idle and detached from the
    /// failed member incarnation.
    async fn restore_resumed_member(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        original_origin: ProvisionSessionOrigin,
        rollback_authority: &ResumedMemberRollbackAuthority,
    ) -> Result<(), MobError>;
    /// Retire the member and report the typed disposal the provisioner
    /// observed (§19.L4): `Archived` when the mob's archive authority owns
    /// and durably archived the session, `RuntimeReleasedOnlyHostOwned` when
    /// the session's durable record belongs to another host and only the
    /// runtime session + binding were released.
    async fn retire_member(
        &self,
        member_ref: &MemberRef,
    ) -> Result<crate::machines::mob_machine::MemberSessionDisposal, MobError>;
    async fn interrupt_member(
        &self,
        member_ref: &MemberRef,
        expected_member: Option<&super::bridge_protocol::BridgeMemberIncarnation>,
    ) -> Result<(), MobError>;
    async fn hard_cancel_member(
        &self,
        member_ref: &MemberRef,
        reason: &str,
    ) -> Result<(), MobError>;
    /// Hard-cancel a PLACED member over the supervisor bridge (DEC-P6E-8).
    /// Placement is the CALLER's machine fact (ADJ-24: never the ref shape)
    /// and the caller has already gated on the recorded host
    /// `hard_cancel_member` capability. Default = typed reject (never a
    /// silent fallback).
    async fn hard_cancel_placed_member(
        &self,
        member_ref: &MemberRef,
        expected_member: &super::bridge_protocol::BridgeMemberIncarnation,
        reason: &str,
    ) -> Result<(), MobError> {
        let _ = (member_ref, expected_member, reason);
        Err(MobError::UnsupportedForMode {
            mode: crate::MobRuntimeMode::TurnDriven,
            reason: "this provisioner serves no placed hard-cancel bridge lane".to_string(),
        })
    }
    /// Level-triggered cancellation of one exact tracked placed input.
    /// Implementations must preserve the full residency fence and return only
    /// an authenticated, replay-stable host receipt; transport failure is
    /// ambiguous and the caller retries this same command without resending
    /// the work input.
    async fn cancel_tracked_placed_input(
        &self,
        member_ref: &MemberRef,
        expected_member: &super::bridge_protocol::BridgeMemberIncarnation,
        input_id: &str,
    ) -> Result<super::bridge_protocol::BridgeTrackedInputCancelResponse, MobError> {
        let _ = (member_ref, expected_member, input_id);
        Err(MobError::UnsupportedForMode {
            mode: crate::MobRuntimeMode::TurnDriven,
            reason: "this provisioner serves no tracked placed-input cancellation lane".to_string(),
        })
    }
    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError>;
    /// `start_turn` variant surfacing the bridge request envelope id for
    /// PLACED members (DEC-P6E-19): the member-side input's `InteractionId`
    /// IS that envelope id, so the pumped interaction terminals carry it and
    /// remote completion waiters can key on it. Local completions have no
    /// envelope — `None` is the local realization, not a fallback.
    async fn start_turn_with_receipt(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<Option<String>, MobError> {
        self.start_turn(member_ref, req).await.map(|()| None)
    }
    /// Start a turn using actor-minted placement authority and stable
    /// interaction correlation for a placed delivery. The payload `input_id`
    /// survives idempotent resend/dedup and is stamped by the member runtime
    /// onto its interaction terminal. Local realizations receive no context
    /// and return `None`.
    async fn start_turn_with_correlation(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
        placed: Option<PlacedTurnDeliveryContext>,
    ) -> Result<Option<String>, MobError> {
        if placed.is_some() {
            return Err(MobError::UnsupportedForMode {
                mode: crate::MobRuntimeMode::TurnDriven,
                reason: "this provisioner does not implement the placed turn correlation lane"
                    .to_string(),
            });
        }
        self.start_turn_with_receipt(member_ref, req).await
    }
    /// Deliver a directive-bearing flow turn to a placed member (§18 O1,
    /// DEC-P6F-4): `DeliverMemberInput` with `payload.turn = Some(directive)`
    /// at the CALLER-minted `input_id`, ADJ-4 single-resend class. Default =
    /// typed reject (never a silent fallback).
    async fn deliver_turn_directive(
        &self,
        member_ref: &MemberRef,
        request: super::remote_flow_ticket::TurnDirectiveDelivery,
    ) -> Result<super::remote_flow_ticket::BridgeDeliveryOutcomeReceipt, MobError> {
        let _ = (member_ref, request);
        Err(MobError::UnsupportedForMode {
            mode: crate::MobRuntimeMode::TurnDriven,
            reason: "turn directives are not supported by this provisioner".to_string(),
        })
    }
    async fn admit_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError>;
    /// Admit a turn and resolve `completion_tx` from the authoritative runtime
    /// completion boundary without holding the mob actor until completion.
    async fn admit_tracked_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
        completion_tx: tokio::sync::oneshot::Sender<Result<(), MobError>>,
        llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
    ) -> Result<(), MobError> {
        let _ = (member_ref, req, completion_tx, llm_identity_applied_tx);
        Err(MobError::UnsupportedForMode {
            mode: crate::MobRuntimeMode::TurnDriven,
            reason: "tracked completion is not supported by this provisioner".to_string(),
        })
    }
    async fn admit_turn_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let _ = operation_id;
        self.admit_turn(member_ref, req).await
    }
    async fn start_flow_step(
        &self,
        member_ref: &MemberRef,
        run_id: &crate::RunId,
        step_id: &crate::StepId,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let _ = (run_id, step_id);
        self.start_turn(member_ref, req).await
    }
    async fn interaction_event_injector(
        &self,
        bridge_session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>>;
    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError>;
    /// Prepare one machine-owned local session for the explicit mob-resume
    /// seam. Returns `true` only when the caller must rebuild the live
    /// session: an exact attachment owned by another provisioner incarnation
    /// was retired, an unattached live actor was discarded, or this
    /// provisioner's exact attachment has lost its actor and must be replaced
    /// through the canonical materialization transaction. Returns `false`
    /// when this provisioner already owns the complete active incarnation (or
    /// when no runtime adapter participates).
    ///
    /// Implementations must never adopt or migrate attachment-local queued
    /// work. Any retirement must be fenced by the machine-minted exact
    /// attachment witness.
    async fn prepare_member_session_for_explicit_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, MobError>;
    async fn ensure_runtime_session_state(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let _ = member_ref;
        Ok(())
    }
    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>>;
    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError>;
    async fn trusted_peer_spec_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let _ = operation_id;
        self.trusted_peer_spec(member_ref, fallback_name, fallback_peer_id)
            .await
    }
    /// Publish operation readiness for an exact endpoint that was already
    /// resolved and durably journaled by the mob spawn owner.
    ///
    /// Implementations must not re-resolve or substitute endpoint material.
    async fn publish_trusted_peer_spec_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        trusted_peer: TrustedPeerDescriptor,
    ) -> Result<(), MobError>;
    async fn reconcile_peer_only_trust(
        &self,
        member_ref: &MemberRef,
        desired_trust: Option<&PeerOnlyTrustOverlay>,
        rebind_authority: Option<&PeerOnlyRebindAuthority>,
    ) -> Result<PeerOnlyTrustReconcileReport, MobError> {
        let _ = (member_ref, desired_trust, rebind_authority);
        Ok(PeerOnlyTrustReconcileReport::default())
    }
    /// Resolve the live canonical mob-child lifecycle operation for an
    /// existing member bridge.
    async fn active_operation_id_for_member(&self, member_ref: &MemberRef) -> Option<OperationId>;
    async fn bind_member_owner_context(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError>;

    /// Rebind a recovered committed placed member to its exact pre-minted
    /// operation.  Unlike the ordinary endpoint-source path this must never
    /// discover or create another operation.
    async fn bind_placed_member_owner_context_exact(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
        display_name: String,
        provision_operation_id: OperationId,
        recovery_expectation: PlacedOperationRecoveryExpectation,
    ) -> Result<(), MobError> {
        let _ = (
            member_ref,
            owner_bridge_session_id,
            ops_registry,
            display_name,
            provision_operation_id,
            recovery_expectation,
        );
        Err(MobError::Internal(
            "this provisioner cannot bind an exact placed operation owner context".to_string(),
        ))
    }

    /// Materialize a member on a bound member host (multi-host §7.3): send
    /// `MaterializeMember` over the supervisor bridge and validate the
    /// transport facts of the typed ack. Default: typed unsupported — only
    /// the bridge-holding provisioner realizes it (DEC-P3-5).
    async fn materialize_member(
        &self,
        req: MaterializeMemberRequest,
    ) -> Result<MaterializedSpawnReceipt, MobError> {
        let _ = req;
        Err(MobError::Internal(
            "this provisioner cannot materialize members on a bound host (no supervisor bridge)"
                .to_string(),
        ))
    }

    /// §19.L3: full durable disposal of a host-materialized member on its
    /// owning host. The ONE disposal verb for placed members; also the
    /// orphan-reconciliation verb (same admission, same reply). Default:
    /// typed unsupported.
    async fn release_host_member(
        &self,
        request: HostMemberReleaseRequest,
    ) -> Result<crate::machines::mob_machine::MemberSessionDisposal, MobError> {
        let _ = request;
        Err(MobError::Internal(
            "this provisioner cannot release host-materialized members (no supervisor bridge)"
                .to_string(),
        ))
    }

    /// Abort the exact pre-dispatch operation anchor after remote member
    /// absence has been certified and before its Pending carrier is deleted.
    /// The registry is re-derived from the carrier's owner session on cold
    /// recovery; `NotFound` and an already-terminal abort are idempotent.
    async fn abort_placed_provision_operation(
        &self,
        operation_owner_session_id: &SessionId,
        provision_operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        let _ = (
            operation_owner_session_id,
            provision_operation_id,
            display_name,
        );
        Err(MobError::Internal(
            "this provisioner cannot resolve a placed operation-owner registry".to_string(),
        ))
    }

    /// Ensure the exact operation named by a committed carrier is Running.
    /// Because nonterminal registry rows are not process-durable, absence is
    /// repaired by registering and starting the SAME id/full tuple;
    /// Provisioning is started and Running is accepted.  This never mints or
    /// discovers another id.
    async fn ensure_committed_placed_provision_operation(
        &self,
        operation_owner_session_id: &SessionId,
        provision_operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        let _ = (
            operation_owner_session_id,
            provision_operation_id,
            display_name,
        );
        Err(MobError::Internal(
            "this provisioner cannot ensure a committed placed operation-owner registry"
                .to_string(),
        ))
    }

    /// Terminalize the exact committed placed operation after durable member
    /// retirement and before carrier deletion.  Missing is restart-idempotent;
    /// this must never reconstruct an active operation.
    async fn retire_committed_placed_provision_operation(
        &self,
        operation_owner_session_id: &SessionId,
        provision_operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        let _ = (
            operation_owner_session_id,
            provision_operation_id,
            display_name,
        );
        Err(MobError::Internal(
            "this provisioner cannot retire a committed placed operation-owner registry"
                .to_string(),
        ))
    }

    /// Cancel all active checkpointer gates so in-flight saves complete but
    /// subsequent checkpoints are no-ops. Call during mob stop.
    async fn cancel_all_checkpointers(&self) {}

    /// Re-enable checkpointer gates after a prior cancel. Call during mob resume.
    async fn rearm_all_checkpointers(&self) {}
}

#[cfg(feature = "runtime-adapter")]
#[derive(Clone)]
pub struct SessionBackend {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<MeerkatMachine>>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    ops_adapter: Arc<super::ops_adapter::MobOpsAdapter>,
    // Capability index for runtime bridge sidecars keyed by registered runtime
    // session identity. This map is never lifecycle truth; canonical
    // registration/attachment truth stays in MeerkatMachine.
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    // DEC-P3H-5: the extracted disposal arc, sharing this backend's
    // `runtime_sessions` sidecar map. The backend delegates its disposal
    // verbs here (one implementation, two instance owners).
    disposal: MemberSessionDisposalArc,
}

#[cfg(feature = "runtime-adapter")]
fn stamp_eager_session_owned_initial_turn_metadata(req: &mut CreateSessionRequest) {
    if req.initial_turn != meerkat_core::service::InitialTurnPolicy::RunImmediately {
        return;
    }
    let build = req
        .build
        .get_or_insert_with(meerkat_core::service::SessionBuildOptions::default);
    build.initial_turn_metadata = Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(
        build.initial_turn_metadata.take(),
    ));
}

/// Typed outcome of one full member-session disposal through
/// [`MemberSessionDisposalArc::dispose`] (DEC-P3H-6, first-serve column).
///
/// `AlreadyArchived` is a SUCCESS-class fact (the durable terminal already
/// holds); machine-side records fold it into `Archived` while the wire keeps
/// the distinction.
#[cfg(feature = "runtime-adapter")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberSessionDisposalVerdict {
    /// Fresh durable archive through the mob lifecycle authority.
    Archived,
    /// The archive authority already holds the terminal (or never had a
    /// record and no live runtime remained) — idempotent success.
    AlreadyArchived,
    /// The session's durable record belongs to another host; only the
    /// runtime session and its binding were released.
    RuntimeReleasedOnlyHostOwned,
}

/// The reusable quiesce → ownership-discriminated archive → runtime
/// unregister disposal arc (DEC-P3H-5), extracted from `SessionBackend` so
/// the member-host materializer can drive the SAME implementation against
/// the daemon's session service. One implementation, two instance owners:
/// `SessionBackend` delegates (behavior byte-identical), and
/// `HostMemberMaterializer` holds its own instance over the daemon's
/// `(service, runtime_adapter)`.
#[cfg(feature = "runtime-adapter")]
#[derive(Clone)]
pub struct MemberSessionDisposalArc {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<MeerkatMachine>>,
    /// Capability index for runtime bridge sidecars (never lifecycle truth).
    /// `SessionBackend` shares its own map; the host materializer shares its
    /// executor-residency sidecar map (ADJ-23) so disposal clears the same
    /// entries the materializer registered.
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
}

#[cfg(feature = "runtime-adapter")]
enum RuntimeSessionDisposalTarget {
    Absent,
    ExactAttachment {
        witness: RuntimeExecutorAttachmentWitness,
        registration: RuntimeSessionRegistrationWitness,
        sidecar: Option<Arc<RuntimeSessionState>>,
    },
    ExactRegistration(RuntimeSessionRegistrationWitness),
}

#[cfg(feature = "runtime-adapter")]
impl MemberSessionDisposalArc {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<MeerkatMachine>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            runtime_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(super) fn with_runtime_sessions(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<MeerkatMachine>>,
        runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            runtime_sessions,
        }
    }

    /// Full member-session disposal with the typed first-serve verdict
    /// (DEC-P3H-6): an archive-authority-owned
    /// `SessionError::NotFound` out of the archive walk means the durable
    /// terminal already holds and no live runtime remained — idempotent
    /// `AlreadyArchived`, never an error. Host-owned absence stays the typed
    /// `RuntimeReleasedOnlyHostOwned` verdict across every retry.
    pub async fn dispose(
        &self,
        session_id: &SessionId,
    ) -> Result<MemberSessionDisposalVerdict, SessionError> {
        // DEC-P3H-6: the archive walk is idempotent, so it reports `Archived`
        // for a terminal that already held (an out-of-band archive). Probe
        // the distinction BEFORE the walk: owned by the archive authority
        // (unfiltered read) while invisible to the archived-filtered read
        // means the durable terminal already holds.
        let already_archived = self
            .session_service
            .session_known_to_archive_authority(session_id)
            .await?
            && self
                .session_service
                .load_persisted_session(session_id)
                .await?
                .is_none();
        match self.archive_with_authority_then_unregister(session_id).await {
            Ok(crate::machines::mob_machine::MemberSessionDisposal::Archived) => {
                Ok(if already_archived {
                    MemberSessionDisposalVerdict::AlreadyArchived
                } else {
                    MemberSessionDisposalVerdict::Archived
                })
            }
            Ok(
                crate::machines::mob_machine::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
            ) => Ok(MemberSessionDisposalVerdict::RuntimeReleasedOnlyHostOwned),
            // The archive walk never reports the non-persistent-backend
            // disposal (that is `release_runtime_only`'s arm); reaching it
            // here is an invariant fault, surfaced typed.
            Ok(
                crate::machines::mob_machine::MemberSessionDisposal::RuntimeReleasedOnlyNoDurableSessions,
            ) => Err(Self::runtime_archive_error(format!(
                "member session disposal for {session_id} reported a non-persistent-backend \
                 verdict from the durable archive walk"
            ))),
            Err(SessionError::NotFound { .. }) => Ok(MemberSessionDisposalVerdict::AlreadyArchived),
            Err(error) => Err(error),
        }
    }

    /// Runtime-retire-only disposal (DEC-P3H-6 non-persistent backend arm):
    /// outer quiesce, durable runtime retire, binding release. No durable
    /// archive exists to write.
    pub async fn release_runtime_only(&self, session_id: &SessionId) -> Result<(), SessionError> {
        let expected_state = self
            .capture_exact_runtime_state_for_disposal(session_id)
            .await?;
        self.cancel_active_runtime_turn_before_retire(session_id)
            .await?;
        let mut boundary =
            RuntimeTurnFinalizationBoundaryLease::acquire(&self.session_service, session_id)
                .await
                .map_err(|error| Self::runtime_archive_error(error.to_string()))?;
        self.retire_runtime_after_turn_boundary(session_id, &mut boundary)
            .await?;
        self.unregister_runtime_session_binding(session_id, &expected_state, boundary)
            .await?;
        Ok(())
    }

    async fn remove_runtime_session_state(
        &self,
        session_id: &SessionId,
        expected: Option<&Arc<RuntimeSessionState>>,
    ) {
        let removed = if let Some(expected) = expected {
            let mut runtime_sessions = self.runtime_sessions.write().await;
            let still_exact = runtime_sessions
                .get(session_id)
                .is_some_and(|current| same_runtime_attachment(current, expected));
            still_exact
                .then(|| runtime_sessions.remove(session_id))
                .flatten()
        } else {
            None
        };
        if let Some(state) = removed {
            state.clear_queued_turns().await;
        }
    }

    async fn capture_exact_runtime_state_for_disposal(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeSessionDisposalTarget, SessionError> {
        let Some(adapter) = &self.runtime_adapter else {
            return Ok(RuntimeSessionDisposalTarget::Absent);
        };
        Self::capture_exact_runtime_state_for_disposal_with_adapter(
            adapter,
            &self.runtime_sessions,
            session_id,
        )
        .await
    }

    async fn capture_exact_runtime_state_for_disposal_with_adapter(
        adapter: &Arc<MeerkatMachine>,
        runtime_sessions: &Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
        session_id: &SessionId,
    ) -> Result<RuntimeSessionDisposalTarget, SessionError> {
        let registration_before = adapter
            .current_session_registration_witness(session_id)
            .await;
        let current = adapter
            .current_executor_attachment_witness(session_id)
            .await;
        let state = runtime_sessions.read().await.get(session_id).cloned();
        let registration = adapter
            .current_session_registration_witness(session_id)
            .await;
        if registration_before != registration {
            return Err(Self::runtime_archive_error(format!(
                "runtime disposal for {session_id} observed a registration replacement while capturing exact attachment state"
            )));
        }
        let retained_cleanup_is_exact = match (&current, &state, &registration) {
            (None, Some(state), Some(registration)) => {
                adapter
                    .executor_attachment_cleanup_is_current_for_registration(
                        state.witness(),
                        registration,
                    )
                    .await
            }
            _ => false,
        };
        match (current, state, registration, retained_cleanup_is_exact) {
            (Some(witness), Some(state), Some(registration), _) if state.witness() == &witness => {
                Ok(RuntimeSessionDisposalTarget::ExactAttachment {
                    witness,
                    registration,
                    sidecar: Some(state),
                })
            }
            // The machine attachment is lifecycle authority. A reconstructed
            // surface can legitimately have no entry in its mechanical
            // sidecar index (for example after a cold backend rebuild or when
            // the attachment was created through another shared surface).
            // Preserve the exact machine witness and retire by compare-and-
            // remove; never require the optional index to manufacture proof.
            (Some(witness), None, Some(registration), _) => {
                Ok(RuntimeSessionDisposalTarget::ExactAttachment {
                    witness,
                    registration,
                    sidecar: None,
                })
            }
            // Canonical unregister hides the serving projection as soon as it
            // enters Draining.  If post-stop cleanup then fails transiently,
            // the exact sidecar remains the retry anchor.  Re-admit it only
            // after the machine proves that both opaque witnesses still name
            // the same registration and retained cleanup attachment.
            (None, Some(state), Some(registration), true) => {
                Ok(RuntimeSessionDisposalTarget::ExactAttachment {
                    witness: state.witness().clone(),
                    registration,
                    sidecar: Some(state),
                })
            }
            (None, None, Some(registration), _) => Ok(
                RuntimeSessionDisposalTarget::ExactRegistration(registration),
            ),
            (None, None, None, _) => Ok(RuntimeSessionDisposalTarget::Absent),
            (current, state, registration, _) => Err(Self::runtime_archive_error(format!(
                "runtime disposal for {session_id} cannot capture an exact machine/sidecar pair: attachment={current:?}, registration={registration:?}, sidecar={:?}",
                state.as_ref().map(|state| state.witness()),
            ))),
        }
    }

    async fn unregister_runtime_session_binding(
        &self,
        session_id: &SessionId,
        target: &RuntimeSessionDisposalTarget,
        boundary: RuntimeTurnFinalizationBoundaryLease,
    ) -> Result<(), SessionError> {
        boundary
            .require_session(session_id)
            .map_err(|error| Self::runtime_archive_error(error.to_string()))?;
        if let Some(adapter) = &self.runtime_adapter {
            match target {
                RuntimeSessionDisposalTarget::Absent => {
                    if adapter.contains_session(session_id).await {
                        return Err(Self::runtime_archive_error(format!(
                            "runtime session {session_id} appeared after its exact absence capture"
                        )));
                    }
                    return Ok(());
                }
                RuntimeSessionDisposalTarget::ExactRegistration(registration) => {
                    let sidecar_present =
                        self.runtime_sessions.read().await.contains_key(session_id);
                    if sidecar_present
                        || adapter
                            .current_executor_attachment_witness(session_id)
                            .await
                            .is_some()
                    {
                        return Err(Self::runtime_archive_error(format!(
                            "runtime session {session_id} acquired attachment-local state before exact registration disposal"
                        )));
                    }
                    // Canonical unregister may enter the service cleanup path.
                    // Release the service boundary before the machine-owned
                    // coordinator acquires its exact registration transaction
                    // and mutation gates.
                    drop(boundary);
                    let removed = adapter
                        .unregister_terminal_session_registration_if_current(registration)
                        .await
                        .map_err(|error| {
                            Self::runtime_archive_error(format!(
                                "failed exact terminal registration disposal for {session_id}: {error}"
                            ))
                        })?;
                    if !removed {
                        return Err(Self::runtime_archive_error(format!(
                            "terminal runtime registration for {session_id} changed before exact disposal"
                        )));
                    }
                    return Ok(());
                }
                RuntimeSessionDisposalTarget::ExactAttachment {
                    witness,
                    registration,
                    sidecar,
                } => {
                    let retirement = adapter
                        .prepare_executor_attachment_retirement_under_runtime_turn_boundary(witness)
                        .await
                        .map_err(|error| {
                            Self::runtime_archive_error(format!(
                                "failed to prepare exact runtime disposal for {session_id}: {error}"
                            ))
                        })?;
                    if let Some(retirement) = retirement {
                        if let Some(expected_state) = sidecar {
                            let exact_sidecar = self
                                .runtime_sessions
                                .read()
                                .await
                                .get(session_id)
                                .is_some_and(|current| {
                                    same_runtime_attachment(current, expected_state)
                                });
                            if !exact_sidecar {
                                return Err(Self::runtime_archive_error(format!(
                                    "runtime sidecar for {session_id} changed before exact disposal"
                                )));
                            }
                            let operation_guard = expected_state.operation_guard().await;
                            expected_state.mark_cleanup_scheduled();
                            expected_state.clear_queued_turns().await;
                            drop(operation_guard);
                        } else if self.runtime_sessions.read().await.contains_key(session_id) {
                            return Err(Self::runtime_archive_error(format!(
                                "runtime sidecar for {session_id} appeared after exact sidecar absence capture"
                            )));
                        }

                        drop(boundary);
                        let removed = retirement
                            .commit()
                            .map_err(|error| {
                                Self::runtime_archive_error(format!(
                                    "failed to start exact runtime disposal for {session_id}: {error}"
                                ))
                            })?
                            .wait()
                            .await
                            .map_err(|error| {
                                Self::runtime_archive_error(format!(
                                    "failed to complete exact runtime disposal for {session_id}: {error}"
                                ))
                            })?;
                        if !removed {
                            return Err(Self::runtime_archive_error(format!(
                                "exact runtime disposal for {session_id} lost its attachment incarnation"
                            )));
                        }
                    } else {
                        // `current_executor_attachment_witness` deliberately
                        // hides an attachment after it enters Draining. Join
                        // the same machine-owned teardown by exact witness
                        // instead of misclassifying that state as replacement.
                        drop(boundary);
                        let removed = adapter
                            .unregister_executor_attachment_if_current(witness)
                            .await
                            .map_err(|error| {
                                Self::runtime_archive_error(format!(
                                    "failed to join exact runtime disposal for {session_id}: {error}"
                                ))
                            })?;
                        if !removed {
                            match adapter
                                .current_session_registration_witness(session_id)
                                .await
                            {
                                None => {}
                                Some(current) if &current == registration => {
                                    let removed = adapter
                                        .unregister_terminal_session_registration_if_current(
                                            registration,
                                        )
                                        .await
                                        .map_err(|error| {
                                            Self::runtime_archive_error(format!(
                                                "failed exact terminal registration disposal for {session_id}: {error}"
                                            ))
                                        })?;
                                    if !removed {
                                        match adapter
                                            .current_session_registration_witness(session_id)
                                            .await
                                        {
                                            None => {}
                                            Some(current) if &current != registration => {
                                                return Err(Self::runtime_archive_error(format!(
                                                    "runtime registration for {session_id} changed before exact disposal"
                                                )));
                                            }
                                            Some(_) => {
                                                return Err(Self::runtime_archive_error(format!(
                                                    "exact terminal runtime registration for {session_id} could not be disposed"
                                                )));
                                            }
                                        }
                                    }
                                }
                                Some(_) => {
                                    return Err(Self::runtime_archive_error(format!(
                                        "runtime registration for {session_id} changed before exact disposal"
                                    )));
                                }
                            }
                        }
                    }
                    if let Some(expected_state) = sidecar {
                        self.remove_runtime_session_state(session_id, Some(expected_state))
                            .await;
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn runtime_archive_error(message: impl Into<String>) -> SessionError {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
            message.into(),
        ))
    }

    async fn retire_runtime_after_turn_boundary(
        &self,
        session_id: &SessionId,
        boundary: &mut RuntimeTurnFinalizationBoundaryLease,
    ) -> Result<(), SessionError> {
        boundary
            .require_session(session_id)
            .map_err(|error| Self::runtime_archive_error(error.to_string()))?;
        let Some(adapter) = &self.runtime_adapter else {
            return Ok(());
        };
        if !adapter.contains_session(session_id).await {
            return Ok(());
        }
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
        let already_terminal = match adapter.meerkat_machine_archive_snapshot(session_id).await {
            Some(snapshot) => matches!(
                snapshot.control.phase,
                meerkat_runtime::RuntimeState::Retired | meerkat_runtime::RuntimeState::Stopped
            ),
            None => return Ok(()),
        };
        if !already_terminal {
            match adapter.retire_runtime_control_plane(&runtime_id).await {
                Ok(_) => {}
                Err(meerkat_runtime::RuntimeControlPlaneError::NotFound(_)) => return Ok(()),
                Err(error) => {
                    return Err(Self::runtime_archive_error(format!(
                        "runtime retire under exact disposal boundary failed for {session_id}: {error}"
                    )));
                }
            }
        }
        self.wait_for_runtime_retire_drain(session_id).await
    }

    fn runtime_run_bound(snapshot: &meerkat_runtime::MeerkatArchiveSnapshot) -> bool {
        snapshot.control.current_run_id.is_some()
    }

    fn runtime_turn_cancellable(snapshot: &meerkat_runtime::MeerkatArchiveSnapshot) -> bool {
        snapshot.control.phase == meerkat_runtime::RuntimeState::Running
            && Self::runtime_run_bound(snapshot)
    }

    async fn cancel_active_runtime_turn_before_retire(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        let Some(adapter) = &self.runtime_adapter else {
            return Ok(());
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        // Cooperative grace: a boundary cancel ends well-behaved turns; a
        // turn parked INSIDE a provider stream never reaches a boundary, so
        // retire/release (which owns the runtime teardown) escalates to the
        // machine-admitted hard cancel — the same user-interrupt authority
        // the explicit `HardCancelMember` verb rides (DEC-P6E-7).
        let escalate_at = Instant::now() + Duration::from_secs(2);
        let mut cancel_requested = false;
        let mut hard_cancel_requested = false;

        loop {
            tracing::info!(
                session_id = %session_id,
                "SessionBackend::cancel_active_runtime_turn_before_retire checking registration"
            );
            if !adapter.contains_session(session_id).await {
                return Ok(());
            }
            tracing::info!(
                session_id = %session_id,
                "SessionBackend::cancel_active_runtime_turn_before_retire loading snapshot"
            );
            let Some(snapshot) = adapter.meerkat_machine_archive_snapshot(session_id).await else {
                return Ok(());
            };
            tracing::info!(
                session_id = %session_id,
                phase = ?snapshot.control.phase,
                control_run = ?snapshot.control.current_run_id,
                queue_len = snapshot.queue.len(),
                steer_queue_len = snapshot.steer_queue.len(),
                "SessionBackend::cancel_active_runtime_turn_before_retire observed snapshot"
            );
            if snapshot.control.phase != meerkat_runtime::RuntimeState::Running
                || !Self::runtime_run_bound(&snapshot)
            {
                return Ok(());
            }

            if !cancel_requested {
                tracing::info!(
                    session_id = %session_id,
                    "SessionBackend::cancel_active_runtime_turn_before_retire requesting boundary cancel"
                );
                if let Err(error) = adapter.cancel_after_boundary(session_id).await {
                    let still_active = adapter
                        .meerkat_machine_archive_snapshot(session_id)
                        .await
                        .is_some_and(|snapshot| Self::runtime_turn_cancellable(&snapshot));
                    if still_active {
                        if matches!(
                            error,
                            meerkat_runtime::RuntimeDriverError::NotReady {
                                state: meerkat_runtime::RuntimeState::Running
                            }
                        ) {
                            cancel_requested = true;
                            continue;
                        }
                        return Err(Self::runtime_archive_error(format!(
                            "runtime cancel-before-retire failed for {session_id}: {error}"
                        )));
                    }
                    continue;
                }
                cancel_requested = true;
            }

            if !hard_cancel_requested && Instant::now() >= escalate_at {
                if let Err(error) = adapter
                    .hard_cancel_current_run(session_id, "member retire requires runtime teardown")
                    .await
                {
                    // NotReady = the turn ended between the snapshot and the
                    // escalation; the loop re-checks and returns.
                    tracing::debug!(
                        session_id = %session_id,
                        error = %error,
                        "hard-cancel escalation before retire rejected; retire wait continues"
                    );
                }
                hard_cancel_requested = true;
            }

            if Instant::now() >= deadline {
                return Err(Self::runtime_archive_error(format!(
                    "timed out waiting for active runtime turn before mob archive for {session_id}: phase={:?}, control_run={:?}, ingress_run={:?}, queue_len={}, steer_queue_len={}",
                    snapshot.control.phase,
                    snapshot.control.current_run_id,
                    snapshot.control.current_run_id,
                    snapshot.queue.len(),
                    snapshot.steer_queue.len(),
                )));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_for_runtime_retire_drain(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        let Some(adapter) = &self.runtime_adapter else {
            return Ok(());
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut terminal_abandon_requested = false;

        loop {
            if !adapter.contains_session(session_id).await {
                return Ok(());
            }
            let Some(snapshot) = adapter.meerkat_machine_archive_snapshot(session_id).await else {
                return Ok(());
            };
            let ingress_quiescent = snapshot.queue.is_empty() && snapshot.steer_queue.is_empty();
            // Retired is machine-owned terminal lifecycle truth for archive
            // purposes. The spine may preserve diagnostic run ids after retire,
            // so unregister must not wait on those ids once ingress is empty.
            let lifecycle_retired = matches!(
                snapshot.control.phase,
                meerkat_runtime::RuntimeState::Retired | meerkat_runtime::RuntimeState::Stopped
            );
            let no_run_bound = snapshot.control.current_run_id.is_none()
                && snapshot.control.current_run_id.is_none();
            let no_completion_waiters = snapshot.completion_waiters.input_count == 0
                && snapshot.completion_waiters.waiter_count == 0;
            if (ingress_quiescent && (lifecycle_retired || no_run_bound))
                || (lifecycle_retired && no_run_bound && no_completion_waiters)
            {
                return Ok(());
            }
            if lifecycle_retired && no_run_bound && !terminal_abandon_requested {
                terminal_abandon_requested = true;
                adapter
                    .abandon_retired_pending_inputs(
                        session_id,
                        "mob archive terminal retire drain cleanup".to_string(),
                    )
                    .await
                    .map_err(|error| {
                        Self::runtime_archive_error(format!(
                            "runtime stop after terminal retire before mob archive failed for {session_id}: {error}"
                        ))
                    })?;
                continue;
            }
            if Instant::now() >= deadline {
                return Err(Self::runtime_archive_error(format!(
                    "timed out waiting for runtime retire drain before mob archive for {session_id}: phase={:?}, control_run={:?}, ingress_run={:?}, queue_len={}, steer_queue_len={}, completion_inputs={}, completion_waiters={}",
                    snapshot.control.phase,
                    snapshot.control.current_run_id,
                    snapshot.control.current_run_id,
                    snapshot.queue.len(),
                    snapshot.steer_queue.len(),
                    snapshot.completion_waiters.input_count,
                    snapshot.completion_waiters.waiter_count,
                )));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn archive_with_authority_then_unregister<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> ArchiveDisposalFuture<'a> {
        Box::pin(async move {
            tracing::info!(
                session_id = %session_id,
                "SessionBackend::archive_with_authority_then_unregister start"
            );

            // Capture durable ownership before retiring the runtime. For a
            // runtime-backed persistent service, retirement deliberately
            // changes the authoritative read source from the runtime
            // snapshot to the durable session-store projection. A freshly
            // materialized deferred member can have an authoritative runtime
            // snapshot before that projection is populated; probing only
            // after Retire therefore misclassifies an owned member as a
            // host-owned session and skips its required archive.
            let archive_authority_owned = self
                .session_service
                .session_known_to_archive_authority(session_id)
                .await?;
            let expected_state = self
                .capture_exact_runtime_state_for_disposal(session_id)
                .await?;
            // Ownership read on the owning authority BEFORE retirement and
            // archiving: a member adopted from a host-owned session service
            // (`MemberLaunchMode::Resume` over a store the mob does not own,
            // e.g. an embedder's console sessions) legitimately has no
            // durable record here. For those, disposal means retiring the
            // runtime session and releasing the binding — the durable
            // session's lifecycle belongs to its host, and archiving through
            // the mob authority is not this mob's to perform. The
            // NotFound-with-registered-runtime escalation below stays fully
            // fail-closed for sessions the authority DOES own (a mid-archive
            // record loss is a genuine split-state, never tolerated).
            if !archive_authority_owned
                && let Some(adapter) = &self.runtime_adapter
                && adapter.contains_session(session_id).await
            {
                self.cancel_active_runtime_turn_before_retire(session_id)
                    .await?;
                let mut boundary = RuntimeTurnFinalizationBoundaryLease::acquire(
                    &self.session_service,
                    session_id,
                )
                .await
                .map_err(|error| Self::runtime_archive_error(error.to_string()))?;
                tracing::info!(
                    session_id = %session_id,
                    "mob archive authority does not own this session's durable record; completing host-owned disposal (runtime retire + binding release)"
                );
                self.retire_runtime_after_turn_boundary(session_id, &mut boundary)
                    .await?;
                self.unregister_runtime_session_binding(session_id, &expected_state, boundary)
                    .await?;
                return Ok(
                    crate::machines::mob_machine::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
                );
            }

            tracing::info!(
                session_id = %session_id,
                "SessionBackend::archive_with_authority_then_unregister archiving session service"
            );
            // Persistent runtime-backed services own the commit-first archive
            // protocol themselves: durable document terminal first, runtime
            // retire second. Quiesce any active turn before entering that
            // protocol, but do not pre-retire the owned session here: doing so
            // destroys the very snapshot that protocol must archive and lets
            // an in-flight RunCompleted race a Retired machine.
            self.cancel_active_runtime_turn_before_retire(session_id)
                .await?;
            let mut boundary =
                RuntimeTurnFinalizationBoundaryLease::acquire(&self.session_service, session_id)
                    .await
                    .map_err(|error| Self::runtime_archive_error(error.to_string()))?;
            match self
                .session_service
                .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
                .await
            {
                Ok(()) => {
                    tracing::info!(
                        session_id = %session_id,
                        "SessionBackend::archive_with_authority_then_unregister unregistering runtime binding"
                    );
                    self.unregister_runtime_session_binding(session_id, &expected_state, boundary)
                        .await?;
                    tracing::info!(
                        session_id = %session_id,
                        "SessionBackend::archive_with_authority_then_unregister complete"
                    );
                    Ok(crate::machines::mob_machine::MemberSessionDisposal::Archived)
                }
                Err(SessionError::NotFound { id }) => {
                    if let Some(adapter) = &self.runtime_adapter
                        && adapter.contains_session(session_id).await
                    {
                        // Ask 21d: the escalation below exists to protect a
                        // LIVE runtime the archive authority cannot see (a
                        // genuine split-state). A runtime in a TERMINAL
                        // lifecycle phase — either an earlier retry retired
                        // it or the service's commit-first archive protocol
                        // reached its runtime-retire step — has no live
                        // obligations left to protect: registration is
                        // un-unregistered residue, and failing here strands
                        // the member in `retiring` on wirings whose archive
                        // authority genuinely never owned the session
                        // (identity-first gateway mob-plane workers).
                        // Complete the disposal instead.
                        use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
                        let runtime_state = adapter.runtime_state(session_id).await;
                        // Retired and Stopped both complete disposal: the
                        // machine's Retire input admits Stopped, so the
                        // durable-retire helper retires a stopped runtime
                        // durably as a machine transition instead of the
                        // former silent early-return. LIVE phases keep the
                        // fail-closed escalation below.
                        if matches!(
                            runtime_state,
                            Ok(meerkat_runtime::RuntimeState::Retired
                                | meerkat_runtime::RuntimeState::Stopped)
                        ) {
                            tracing::info!(
                                session_id = %session_id,
                                state = ?runtime_state,
                                "mob archive authority returned NotFound for a terminal-phase registered runtime; completing disposal"
                            );
                            self.retire_runtime_after_turn_boundary(session_id, &mut boundary)
                                .await?;
                            self.unregister_runtime_session_binding(
                                session_id,
                                &expected_state,
                                boundary,
                            )
                            .await?;
                            if archive_authority_owned {
                                // Owned AlreadyArchived is represented by the
                                // service's public NotFound contract. Preserve
                                // that success-class distinction for the
                                // caller instead of laundering it into the
                                // host-owned degradation.
                                return Err(SessionError::NotFound { id });
                            }
                            // No archived document exists (the authority
                            // NotFound'd): the durable session belongs to its
                            // host — the same host-owned disposal fact as the
                            // pre-archive ownership-probe arm above.
                            return Ok(
                                crate::machines::mob_machine::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
                            );
                        }
                        return Err(SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "mob archive authority returned NotFound for registered runtime session {session_id}"
                            )),
                        ));
                    }
                    self.unregister_runtime_session_binding(session_id, &expected_state, boundary)
                        .await?;
                    if archive_authority_owned {
                        // Preserve the owned authority's public NotFound
                        // contract. `dispose` maps it to the idempotent
                        // AlreadyArchived success class.
                        Err(SessionError::NotFound { id })
                    } else {
                        // A first host-owned disposal unregisters the runtime,
                        // so every retry reaches this no-registration branch.
                        // Durable ownership has not changed: retain the exact
                        // runtime-only verdict instead of laundering the retry
                        // into AlreadyArchived merely because cleanup already
                        // removed the adapter binding.
                        Ok(
                            crate::machines::mob_machine::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
                        )
                    }
                }
                Err(error) => Err(error),
            }
        })
    }
}

#[cfg(feature = "runtime-adapter")]
pub(super) enum FailedResumeRuntimeAuthority<'a> {
    ExactAttachment(&'a Arc<RuntimeSessionState>),
    ExactPrepared(&'a mut PreparedSessionMaterialization),
}

#[cfg(feature = "runtime-adapter")]
impl SessionBackend {
    /// Replace one retained serving attachment at the explicit mob-resume seam.
    ///
    /// A newly reconstructed mob owns a fresh attachment-local sidecar map, so
    /// it cannot adopt an executor minted for the prior mob actor. Retire that
    /// exact incarnation through the machine-owned teardown saga; its post-stop
    /// cleanup removes only the old actor/sidecar and clears only old queued
    /// work. The caller can then use the ordinary `ResumedDurable` provision
    /// path to mint a new actor, attachment, and sidecar.
    async fn retire_exact_attachment_for_explicit_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, MobError> {
        let Some(adapter) = self.runtime_adapter.as_ref() else {
            // Runtime-less session services retain their existing live actor;
            // there is no attachment-local sidecar to replace.
            return Ok(false);
        };
        let boundary =
            RuntimeTurnFinalizationBoundaryLease::acquire(&self.session_service, session_id)
                .await?;
        let witness = adapter
            .current_executor_attachment_witness(session_id)
            .await;
        // B stabilizes the actor/attachment incarnation while the sidecar is
        // sampled. Do not retain R while acquiring M below. An exact active
        // sidecar in this backend proves same-handle ownership and must be
        // reused rather than torn down.
        let observed_sidecar = self.runtime_sessions.read().await.get(session_id).cloned();
        if let (Some(witness), Some(sidecar)) = (&witness, &observed_sidecar)
            && sidecar.witness() == witness
        {
            if !sidecar.attachment_is_active(witness) || sidecar.cleanup_scheduled() {
                return Err(MobError::Internal(format!(
                    "explicit resume found its exact attachment sidecar for '{session_id}' but that sidecar is not active and reusable"
                )));
            }
            let exact_actor_live = sidecar
                .actor_witness()
                .is_some_and(|witness| witness.is_live())
                && self
                    .session_service
                    .live_session_actor_registered(session_id)
                    .await?;
            if exact_actor_live {
                return Ok(false);
            }
            // The machine attachment and sidecar still agree, but their
            // exact actor incarnation was revoked or exited. This is not a
            // reusable live incarnation: retire it below. Its ops registry is
            // attachment-local machine plumbing as well, so the exact old
            // binding is compare-removed after retirement and the ordinary
            // explicit-resume materialization mints a coherent replacement.
        }
        let observed_ops_binding = self.ops_adapter.capture_session_binding_witness(session_id);

        let Some(witness) = witness else {
            // `active_ids` was observed before B. If the actor outlived its
            // attachment (or the exact attachment disappeared while resume
            // acquired B), the fresh backend still cannot adopt that actor.
            // Explicit resume is authorized to discard it and reconstruct the
            // exact actor + attachment pair through `ResumedDurable`.
            let discard_result = self
                .session_service
                .discard_live_session_under_runtime_turn_boundary(session_id)
                .await;
            drop(boundary);
            match discard_result {
                Ok(()) | Err(SessionError::NotFound { .. }) => {
                    self.remove_runtime_session_state(session_id, observed_sidecar.as_ref())
                        .await;
                    self.ops_adapter.clear_session_binding_for_explicit_resume(
                        session_id,
                        observed_ops_binding,
                    )?;
                    return Ok(true);
                }
                Err(error) => return Err(error.into()),
            }
        };
        let retirement = adapter
            .prepare_executor_attachment_retirement_under_runtime_turn_boundary(&witness)
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "failed to prepare exact explicit-resume takeover for '{session_id}': {error}"
                ))
            })?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "exact attachment for '{session_id}' changed before explicit-resume takeover"
                ))
            })?;

        // The retirement lease retains exact M. Release B before the owned
        // teardown closes the runtime loop, whose canonical post-stop cleanup
        // reacquires B to remove the old actor and sidecar.
        drop(boundary);
        let removed = retirement
            .commit()
            .map_err(|error| {
                MobError::Internal(format!(
                    "failed to start exact explicit-resume takeover for '{session_id}': {error}"
                ))
            })?
            .wait()
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "failed to complete exact explicit-resume takeover for '{session_id}': {error}"
                ))
            })?;
        if !removed {
            return Err(MobError::Internal(format!(
                "exact attachment for '{session_id}' changed during explicit-resume takeover"
            )));
        }
        // A stale sidecar in this provisioner can only belong to an older
        // witness. Compare-and-remove it after the exact current retirement;
        // a concurrently installed replacement can never be removed here.
        self.remove_runtime_session_state(session_id, observed_sidecar.as_ref())
            .await;
        self.ops_adapter
            .clear_session_binding_for_explicit_resume(session_id, observed_ops_binding)?;
        Ok(true)
    }

    #[cfg(feature = "runtime-adapter")]
    pub(super) async fn restore_failed_resume_before_receipt(
        &self,
        session_id: &SessionId,
        restore_retired: bool,
        mut authority: FailedResumeRuntimeAuthority<'_>,
        boundary: &mut RuntimeTurnFinalizationBoundaryLease,
    ) -> Result<(), MobError> {
        boundary.require_session(session_id)?;
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "resume rollback for '{session_id}' requires runtime authority"
            ))
        })?;
        match &mut authority {
            FailedResumeRuntimeAuthority::ExactAttachment(expected_state) => {
                let operation_guard = expected_state.operation_guard().await;
                let current = adapter
                    .current_executor_attachment_witness(session_id)
                    .await;
                let exact_sidecar = self
                    .runtime_sessions
                    .read()
                    .await
                    .get(session_id)
                    .is_some_and(|current| same_runtime_attachment(current, expected_state));
                if current.as_ref() != Some(expected_state.witness()) || !exact_sidecar {
                    return Err(MobError::Internal(format!(
                        "resume rollback for '{session_id}' lost its exact runtime attachment or sidecar before durable convergence"
                    )));
                }
                expected_state.mark_cleanup_scheduled();
                expected_state.clear_queued_turns().await;
                // Admission takes R before asking the machine for M. Marking
                // this exact sidecar Retiring while R is held prevents every
                // later admission from reaching M; release R before acquiring
                // the exact retirement lease so an already-admitted R -> M
                // operation can finish and the post-stop callback can acquire
                // R without either inversion or self-deadlock.
                drop(operation_guard);
            }
            FailedResumeRuntimeAuthority::ExactPrepared(prepared) => {
                if prepared.session_id() != session_id
                    || !prepared.owns_current_materialization_claim().await
                {
                    return Err(MobError::Internal(format!(
                        "resume rollback for '{session_id}' lost its exact prepared materialization claim before durable convergence"
                    )));
                }
                if self.runtime_sessions.read().await.contains_key(session_id) {
                    return Err(MobError::Internal(format!(
                        "resume rollback for '{session_id}' found a sidecar without a committed exact attachment"
                    )));
                }
            }
        }
        if restore_retired {
            // A retired-session revival crosses two durable authorities before
            // executor attachment: Archived -> Active, then Retired -> Idle.
            // Archive owns the ordered durable convergence: write the Active
            // document back to Archived first, then retire the same machine
            // runtime while B still excludes actor replacement. Do not fully
            // unregister the exact attachment before this protocol consumes
            // its runtime authority.
            match self
                .session_service
                .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
                .await
            {
                Ok(()) | Err(SessionError::NotFound { .. }) => {}
                Err(error) => {
                    return Err(MobError::Internal(format!(
                        "failed to restore revived session '{session_id}' to Archived+Retired: {error}"
                    )));
                }
            }
        }

        match authority {
            FailedResumeRuntimeAuthority::ExactAttachment(expected_state) => {
                let retirement = adapter
                    .prepare_executor_attachment_retirement_under_runtime_turn_boundary(
                        expected_state.witness(),
                    )
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to acquire exact resumed attachment retirement for '{session_id}': {error}"
                        ))
                    })?;
                if let Some(retirement) = retirement {
                    let removed = retirement
                        .commit_under_runtime_turn_finalization_boundary()
                        .await
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "failed to retire exact resumed attachment for '{session_id}': {error}"
                            ))
                        })?;
                    if !removed {
                        return Err(MobError::Internal(format!(
                            "resumed attachment for '{session_id}' changed during exact retirement"
                        )));
                    }
                } else if adapter
                    .current_executor_attachment_witness(session_id)
                    .await
                    .as_ref()
                    == Some(expected_state.witness())
                {
                    return Err(MobError::Internal(format!(
                        "resumed attachment for '{session_id}' remained current but could not be retired"
                    )));
                }
                self.remove_runtime_session_state(session_id, Some(expected_state))
                    .await;
            }
            FailedResumeRuntimeAuthority::ExactPrepared(_) => {
                match self
                    .session_service
                    .discard_live_session_under_runtime_turn_boundary(session_id)
                    .await
                {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }

        if restore_retired {
            let restored = self
                .session_service
                .load_revivable_retired_session(session_id)
                .await?
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "revived session rollback did not restore retired session '{session_id}'"
                    ))
                })?;
            if restored.lifecycle_terminal()
                != Some(meerkat_core::session::SessionLifecycleTerminal::Archived)
            {
                return Err(MobError::Internal(format!(
                    "revived session rollback did not restore archived document '{session_id}'"
                )));
            }

            return Ok(());
        }
        Ok(())
    }

    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<MeerkatMachine>>,
        workgraph_service: Option<meerkat::WorkGraphService>,
    ) -> Self {
        let runtime_sessions = Arc::new(RwLock::new(HashMap::new()));
        let disposal = MemberSessionDisposalArc::with_runtime_sessions(
            Arc::clone(&session_service),
            runtime_adapter.clone(),
            Arc::clone(&runtime_sessions),
        );
        Self {
            session_service,
            runtime_adapter,
            workgraph_service,
            ops_adapter: Arc::new(super::ops_adapter::MobOpsAdapter::new()),
            runtime_sessions,
            disposal,
        }
    }

    fn require_session(
        member_ref: &MemberRef,
        operation: &'static str,
    ) -> Result<SessionId, MobError> {
        member_ref.bridge_session_id().cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "session-backed provisioner cannot {operation} member without session bridge: {member_ref:?}"
            ))
        })
    }

    fn trusted_peer_spec(
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let pubkey =
            meerkat_comms::PubKey::from_pubkey_string(fallback_peer_id).map_err(|err| {
                MobError::WiringError(format!(
                    "invalid peer spec for '{fallback_name}': ed25519 public key required: {err}"
                ))
            })?;
        let derived_peer_id = pubkey.to_peer_id().to_string();
        TrustedPeerDescriptor::unsigned_with_pubkey(
            fallback_name,
            derived_peer_id,
            *pubkey.as_bytes(),
            format!("inproc://{fallback_name}"),
        )
        .map_err(|error| MobError::WiringError(format!("invalid peer spec: {error}")))
    }

    pub(super) fn trusted_peer_spec_from_runtime(
        fallback_name: &str,
        runtime: &dyn CoreCommsRuntime,
    ) -> Result<Option<TrustedPeerDescriptor>, MobError> {
        let Some(peer_id) = runtime.peer_id() else {
            return Ok(None);
        };
        let Some(pubkey) = runtime.public_key_bytes() else {
            return Ok(None);
        };
        TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer_id, &pubkey).map_err(|error| {
            MobError::WiringError(format!("invalid peer spec for '{fallback_name}': {error}"))
        })?;
        let name = runtime
            .comms_name()
            .unwrap_or_else(|| fallback_name.to_string());
        let name = PeerName::new(name).map_err(|error| {
            MobError::WiringError(format!(
                "invalid peer spec for '{fallback_name}': invalid peer name: {error}"
            ))
        })?;
        let address = runtime
            .advertised_address()
            .unwrap_or_else(|| format!("inproc://{fallback_name}"));
        let address = PeerAddress::parse(&address).map_err(|error| {
            MobError::WiringError(format!(
                "invalid peer spec for '{fallback_name}': invalid peer address: {error}"
            ))
        })?;
        Ok(Some(TrustedPeerDescriptor {
            peer_id,
            name,
            address,
            pubkey,
        }))
    }

    async fn runtime_session_state(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Arc<RuntimeSessionState>>, MobError> {
        let Some(adapter) = self.runtime_adapter.as_ref() else {
            return Ok(None);
        };
        if let Some(witness) = adapter
            .current_executor_attachment_witness(session_id)
            .await
        {
            if let Some(existing) = self.runtime_sessions.read().await.get(session_id).cloned()
                && existing.attachment_is_active(&witness)
            {
                return Ok(Some(existing));
            }
            return Err(MobError::Internal(format!(
                "session '{session_id}' has a committed runtime attachment without its exact mob sidecar; explicit resume is required"
            )));
        }
        Err(MobError::Internal(format!(
            "session '{session_id}' has no committed runtime attachment; explicit create or resume is required"
        )))
    }

    #[cfg(test)]
    pub(super) async fn has_runtime_session_sidecar_for_test(
        &self,
        session_id: &SessionId,
    ) -> bool {
        self.runtime_sessions.read().await.contains_key(session_id)
    }

    async fn capture_runtime_session_state(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<RuntimeSessionState>> {
        self.runtime_sessions.read().await.get(session_id).cloned()
    }

    async fn remove_runtime_session_state(
        &self,
        session_id: &SessionId,
        expected: Option<&Arc<RuntimeSessionState>>,
    ) {
        self.disposal
            .remove_runtime_session_state(session_id, expected)
            .await;
    }

    fn archive_with_authority_then_unregister<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> ArchiveDisposalFuture<'a> {
        self.disposal
            .archive_with_authority_then_unregister(session_id)
    }

    async fn execute_runtime_input(
        &self,
        session_id: &SessionId,
        input: Input,
        event_tx: Option<TurnEventTx>,
    ) -> Result<(), MobError> {
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "runtime-backed turn requested without runtime adapter: {session_id}"
            ))
        })?;
        let state = self
            .runtime_session_state(session_id)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "runtime-backed turn has no attachment state: {session_id}"
                ))
            })?;
        let requested_input_id = input.id().clone();
        let (queued_event_tx, deferred_delivery) =
            defer_turn_events_until_machine_completion(session_id, event_tx);
        let operation_guard = state.exact_operation_guard(adapter, session_id).await?;
        // The registration is request-scoped: cancellation at any later await
        // synchronously removes the exact attachment-local context.
        let mut queued_context =
            state.register_turn_context(requested_input_id.clone(), queued_event_tx, None)?;

        let (outcome, handle) = match adapter
            .accept_input_with_completion_for_attachment(state.witness(), input)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                drop(queued_context);
                drop(operation_guard);
                let error = err.to_string();
                if let Some(delivery) = deferred_delivery {
                    delivery.release(DeferredTurnEventOutcome::Failed);
                }
                return Err(MobError::Internal(error));
            }
        };
        tracing::debug!(
            session_id = %session_id,
            input_id = %requested_input_id,
            has_handle = handle.is_some(),
            "SessionBackend::admit_runtime_input accepted input with completion"
        );

        if let Some(queued_context) = queued_context.as_mut() {
            let canonical_input_id = match &outcome {
                meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => Some(input_id),
                // For in-flight dedup, runtime primitives are keyed by the existing
                // canonical input id, not the newly attempted one.
                meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => {
                    Some(existing_id)
                }
                _ => None,
            };
            if let Some(input_id) = canonical_input_id
                && input_id != &requested_input_id
            {
                let _ = queued_context.rekey(input_id.clone());
            }
        }
        drop(operation_guard);

        // Terminal dedup: input already processed — idempotent success
        let Some(handle) = handle else {
            if let Some(queued_context) = queued_context.as_mut() {
                queued_context.resolve_without_execution(None);
            }
            drop(queued_context);
            if let Some(delivery) = deferred_delivery {
                delivery.release(DeferredTurnEventOutcome::Succeeded);
            }
            return Ok(());
        };

        let completion = handle.wait().await;
        drop(queued_context);
        let delivery_outcome = deferred_turn_outcome_from_completion(&completion);
        let result = runtime_completion_to_mob_result(session_id, completion);
        if let Some(delivery) = deferred_delivery {
            delivery.release(delivery_outcome);
        }
        result
    }

    fn runtime_input_from_turn_request(req: &StartTurnRequest) -> Input {
        // The canonical `RuntimeTurnMetadata` carrier owns render metadata and
        // skill references; only handling/overlay retain a flat fallback on
        // `StartTurnRuntimeSemantics`.
        let mut turn_metadata = req.runtime.turn_metadata.clone().unwrap_or_default();
        turn_metadata.handling_mode = Some(
            turn_metadata
                .handling_mode
                .unwrap_or(req.runtime.handling_mode),
        );
        if turn_metadata.turn_tool_overlay.is_none() {
            turn_metadata.turn_tool_overlay = req.runtime.turn_tool_overlay.clone();
        }
        let prompt = req.prompt.clone();
        Input::Prompt(PromptInput {
            header: InputHeader {
                id: meerkat_core::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: prompt,
            typed_turn_appends: req.runtime.typed_turn_appends.clone(),
            // Runtime-backed members lower injected context through the
            // prompt input's typed slot so the runtime batch places the
            // InjectedContext-role appends before the user append — the
            // request field must not also reach the runner (one lowering
            // owner per mode).
            injected_context: req.injected_context.clone(),
            turn_metadata: Some(turn_metadata),
        })
    }

    async fn admit_runtime_input(
        &self,
        session_id: &SessionId,
        input: Input,
        event_tx: Option<TurnEventTx>,
        completion_tx: Option<oneshot::Sender<Result<(), MobError>>>,
        llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
    ) -> Result<(), MobError> {
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "runtime-backed turn requested without runtime adapter: {session_id}"
            ))
        })?;
        let state = self
            .runtime_session_state(session_id)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "runtime-backed turn has no attachment state: {session_id}"
                ))
            })?;
        let requested_input_id = input.id().clone();
        // A live Steer is admitted into the active run rather than lowered to
        // a serialized executor primitive. The actor rejects Steer requests
        // carrying an LLM identity override before admission, so the tracked
        // identity acknowledgement for this lane is authoritatively
        // `Ok(None)` as soon as runtime admission succeeds. Keep the rest of
        // the queued context alive for event/completion delivery.
        let (queued_event_tx, deferred_delivery) =
            defer_turn_events_until_machine_completion(session_id, event_tx);

        #[cfg(target_arch = "wasm32")]
        {
            // Browser WASM runs the mob actor and runtime adapter on the same
            // single-thread executor. Polling this admission while the actor
            // command is still active starves the spawned runtime task, so the
            // standalone browser substrate schedules admission across the
            // actor boundary. Native runtime-backed paths keep the stricter
            // synchronous admission result below.
            let adapter = Arc::clone(adapter);
            let state = Arc::clone(&state);
            let task_session_id = session_id.clone();
            let task_input_id = requested_input_id;
            let resolves_llm_identity_at_admission =
                input.handling_mode() == Some(meerkat_core::types::HandlingMode::Steer);
            tokio::spawn(async move {
                let admission = async {
                    let operation_guard = state
                        .exact_operation_guard(&adapter, &task_session_id)
                        .await?;
                    let mut queued_context = state.register_turn_context(
                        task_input_id.clone(),
                        queued_event_tx,
                        llm_identity_applied_tx,
                    )?;
                    let (outcome, handle) = adapter
                        .accept_input_with_completion_for_attachment(state.witness(), input)
                        .await
                        .map_err(|error| MobError::Internal(error.to_string()))?;
                    if let Some(queued_context) = queued_context.as_mut() {
                        let canonical_input_id = match &outcome {
                            meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => {
                                Some(input_id)
                            }
                            meerkat_runtime::AcceptOutcome::Deduplicated {
                                existing_id, ..
                            } => Some(existing_id),
                            _ => None,
                        };
                        if let Some(input_id) = canonical_input_id
                            && input_id != &task_input_id
                        {
                            let _ = queued_context.rekey(input_id.clone());
                        }
                        if resolves_llm_identity_at_admission {
                            queued_context.resolve_llm_identity_at_admission(None);
                        }
                    }
                    drop(operation_guard);
                    Ok::<_, MobError>((handle, queued_context))
                }
                .await;

                let (result, delivery_outcome) = match admission {
                    Ok((Some(handle), queued_context)) => {
                        let completion = handle.wait().await;
                        drop(queued_context);
                        let delivery_outcome = deferred_turn_outcome_from_completion(&completion);
                        (
                            runtime_completion_to_mob_result(&task_session_id, completion),
                            delivery_outcome,
                        )
                    }
                    Ok((None, mut queued_context)) => {
                        if let Some(queued_context) = queued_context.as_mut() {
                            queued_context.resolve_without_execution(None);
                        }
                        drop(queued_context);
                        (Ok(()), DeferredTurnEventOutcome::Succeeded)
                    }
                    Err(error) => (Err(error), DeferredTurnEventOutcome::Failed),
                };
                if let Some(delivery) = deferred_delivery {
                    delivery.release(delivery_outcome);
                }
                if let Some(completion_tx) = completion_tx {
                    let _ = completion_tx.send(result);
                } else if let Err(error) = result {
                    tracing::warn!(
                        session_id = %task_session_id,
                        input_id = %task_input_id,
                        %error,
                        "background WASM runtime turn failed"
                    );
                }
            });
            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let resolves_llm_identity_at_admission =
                input.handling_mode() == Some(meerkat_core::types::HandlingMode::Steer);
            let operation_guard = state.exact_operation_guard(adapter, session_id).await?;
            let mut queued_context = state.register_turn_context(
                requested_input_id.clone(),
                queued_event_tx,
                llm_identity_applied_tx,
            )?;
            let accept_result = adapter
                .accept_input_with_completion_for_attachment(state.witness(), input)
                .await;

            let (outcome, handle) = match accept_result {
                Ok(result) => result,
                Err(err) => {
                    drop(queued_context);
                    drop(operation_guard);
                    let error = err.to_string();
                    if let Some(delivery) = deferred_delivery {
                        delivery.release(DeferredTurnEventOutcome::Failed);
                    }
                    return Err(MobError::Internal(error));
                }
            };

            if let Some(queued_context) = queued_context.as_mut() {
                let canonical_input_id = match &outcome {
                    meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => Some(input_id),
                    meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => {
                        Some(existing_id)
                    }
                    _ => None,
                };
                if let Some(input_id) = canonical_input_id
                    && input_id != &requested_input_id
                {
                    let _ = queued_context.rekey(input_id.clone());
                }
                if resolves_llm_identity_at_admission {
                    queued_context.resolve_llm_identity_at_admission(None);
                }
            }
            drop(operation_guard);

            let Some(handle) = handle else {
                if let Some(queued_context) = queued_context.as_mut() {
                    queued_context.resolve_without_execution(None);
                }
                drop(queued_context);
                if let Some(delivery) = deferred_delivery {
                    delivery.release(DeferredTurnEventOutcome::Succeeded);
                }
                if let Some(completion_tx) = completion_tx {
                    let _ = completion_tx.send(Ok(()));
                }
                return Ok(());
            };

            let completion_session_id = session_id.clone();
            tokio::spawn(async move {
                let completion = handle.wait().await;
                drop(queued_context);
                let delivery_outcome = deferred_turn_outcome_from_completion(&completion);
                let result = runtime_completion_to_mob_result(&completion_session_id, completion);
                if let Some(delivery) = deferred_delivery {
                    delivery.release(delivery_outcome);
                }
                if let Some(completion_tx) = completion_tx {
                    let _ = completion_tx.send(result);
                }
            });

            Ok(())
        }
    }
}

#[cfg(feature = "runtime-adapter")]
fn runtime_completion_to_mob_result(
    session_id: &SessionId,
    completion: Result<
        meerkat_runtime::completion::CompletionOutcome,
        meerkat_runtime::completion::CompletionWaitError,
    >,
) -> Result<(), MobError> {
    let completion = completion.map_err(|error| {
        MobError::Internal(format!("runtime completion waiter failed: {error}"))
    })?;
    match completion {
        meerkat_runtime::completion::CompletionOutcome::Completed(_) => Ok(()),
        meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => Ok(()),
        meerkat_runtime::completion::CompletionOutcome::CallbackPending { tool_name, args } => {
            Err(MobError::CallbackPending {
                session_id: session_id.clone(),
                tool_name,
                args,
            })
        }
        meerkat_runtime::completion::CompletionOutcome::Cancelled => {
            Err(MobError::Internal("turn cancelled".to_string()))
        }
        meerkat_runtime::completion::CompletionOutcome::Abandoned { reason, .. } => {
            Err(MobError::Internal(format!("turn abandoned: {reason}")))
        }
        meerkat_runtime::completion::CompletionOutcome::AbandonedWithError { reason, error } => {
            Err(MobError::Internal(format!(
                "turn abandoned: {reason}; error={}",
                serde_json::to_string(&error).unwrap_or_else(|_| "<unserializable>".to_string())
            )))
        }
        meerkat_runtime::completion::CompletionOutcome::CompletedWithFinalizationFailure {
            error,
        } => Err(MobError::Internal(format!(
            "turn finalization failed after output: {}",
            error
                .detail
                .as_deref()
                .unwrap_or("turn finalization failed"),
        ))),
        meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { reason, .. } => {
            Err(MobError::Internal(format!("runtime terminated: {reason}")))
        }
    }
}

#[cfg(feature = "runtime-adapter")]
fn deferred_turn_outcome_from_completion(
    completion: &Result<
        meerkat_runtime::completion::CompletionOutcome,
        meerkat_runtime::completion::CompletionWaitError,
    >,
) -> DeferredTurnEventOutcome {
    match completion {
        Ok(meerkat_runtime::completion::CompletionOutcome::Completed(result))
            if result.extraction_error.is_some() =>
        {
            DeferredTurnEventOutcome::ExtractionFailed
        }
        Ok(
            meerkat_runtime::completion::CompletionOutcome::Completed(_)
            | meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult,
        ) => DeferredTurnEventOutcome::Succeeded,
        Ok(meerkat_runtime::completion::CompletionOutcome::CallbackPending { .. }) => {
            DeferredTurnEventOutcome::CallbackPending
        }
        Ok(
            meerkat_runtime::completion::CompletionOutcome::Cancelled
            | meerkat_runtime::completion::CompletionOutcome::Abandoned { .. }
            | meerkat_runtime::completion::CompletionOutcome::AbandonedWithError { .. }
            | meerkat_runtime::completion::CompletionOutcome::CompletedWithFinalizationFailure {
                ..
            }
            | meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { .. },
        )
        | Err(_) => DeferredTurnEventOutcome::Failed,
    }
}

fn session_turn_error_to_mob_error(bridge_session_id: &SessionId, error: SessionError) -> MobError {
    match error {
        SessionError::Agent(meerkat_core::error::AgentError::CallbackPending {
            tool_name,
            args,
        }) => MobError::CallbackPending {
            session_id: bridge_session_id.clone(),
            tool_name,
            args,
        },
        other => other.into(),
    }
}

#[cfg(feature = "runtime-adapter")]
pub(super) struct RuntimeSessionState {
    // Exact machine-owned attachment identity for this sidecar.
    attachment_witness: RuntimeExecutorAttachmentWitness,
    // Exact service-owned actor incarnation bound to this attachment. `None`
    // exists only for the fail-closed synthetic-service test constructor.
    actor_witness: StdMutex<Option<meerkat_session::LiveSessionActorWitness>>,
    // Mechanical attachment-local plumbing phase. This is deliberately not a
    // lifecycle authority: MeerkatMachine remains the authority for whether
    // the executor exists and may serve.
    attachment_phase: std::sync::atomic::AtomicU8,
    // Serializes exact-witness validation, request-context registration, and
    // machine admission against attachment-local retirement.
    operation_gate: Arc<Mutex<()>>,
    // Transport-only request context keyed by canonical runtime input identity.
    // This owner is minted with, and never outlives or transfers away from,
    // this exact attachment. Replacement attachments receive a fresh owner;
    // retirement of A drops A's queued request contexts instead of exposing
    // them to B. It remains mechanical plumbing, never lifecycle truth.
    queued_turn_owner: Arc<RuntimeQueuedTurnOwner>,
}

#[cfg(feature = "runtime-adapter")]
const RUNTIME_ATTACHMENT_PENDING: u8 = 0;
#[cfg(feature = "runtime-adapter")]
const RUNTIME_ATTACHMENT_ACTIVE: u8 = 1;
#[cfg(feature = "runtime-adapter")]
const RUNTIME_ATTACHMENT_RETIREMENT_UNCERTAIN: u8 = 2;
#[cfg(feature = "runtime-adapter")]
const RUNTIME_ATTACHMENT_RETIRING: u8 = 3;
#[cfg(feature = "runtime-adapter")]
const RUNTIME_ATTACHMENT_RETIRED: u8 = 4;

#[cfg(feature = "runtime-adapter")]
#[derive(Default)]
struct RuntimeSessionQueue {
    // Event delivery transport handles for runtime-backed turn dispatch.
    entries: HashMap<InputId, QueuedTurnContext>,
}

/// Attachment-bound owner of request-scoped turn delivery context.
///
/// The exact attachment witness is a compare-and-remove fence, not shadow
/// lifecycle truth. Registration and delivery require that same witness, and
/// cleanup for attachment A can clear only A's contexts. Replacement B is
/// minted with a distinct, initially empty owner; requests are never silently
/// transferred between attachments.
#[cfg(feature = "runtime-adapter")]
struct RuntimeQueuedTurnOwner {
    state: StdMutex<RuntimeQueuedTurnOwnerState>,
}

#[cfg(feature = "runtime-adapter")]
struct RuntimeQueuedTurnOwnerState {
    attachment_witness: RuntimeExecutorAttachmentWitness,
    queue: RuntimeSessionQueue,
}

#[cfg(feature = "runtime-adapter")]
struct QueuedTurnContext {
    event_txs: Vec<TurnEventTx>,
    llm_identity_application: PendingMemberTurnLlmIdentityApplication,
}

#[cfg(feature = "runtime-adapter")]
impl QueuedTurnContext {
    fn new(
        event_tx: Option<TurnEventTx>,
        llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
    ) -> Self {
        Self {
            event_txs: event_tx.into_iter().collect(),
            llm_identity_application: PendingMemberTurnLlmIdentityApplication::new(
                llm_identity_applied_tx,
            ),
        }
    }

    fn append(&mut self, mut other: Self) {
        self.event_txs.append(&mut other.event_txs);
        self.llm_identity_application
            .append(&mut other.llm_identity_application);
    }

    fn into_executor_parts(self) -> (Option<TurnEventTx>, PendingMemberTurnLlmIdentityApplication) {
        (
            fan_out_turn_event_txs(self.event_txs),
            self.llm_identity_application,
        )
    }
}

#[cfg(feature = "runtime-adapter")]
#[derive(Default)]
struct PendingMemberTurnLlmIdentityApplication {
    senders: Vec<super::handle::MemberTurnLlmIdentityAppliedSender>,
}

#[cfg(feature = "runtime-adapter")]
impl PendingMemberTurnLlmIdentityApplication {
    fn new(sender: Option<super::handle::MemberTurnLlmIdentityAppliedSender>) -> Self {
        Self {
            senders: sender.into_iter().collect(),
        }
    }

    fn append(&mut self, other: &mut Self) {
        self.senders.append(&mut other.senders);
    }

    fn resolve_success(&mut self, identity: Option<meerkat_core::SessionLlmIdentity>) {
        for sender in self.senders.drain(..) {
            let _ = sender.send(Ok(identity.clone()));
        }
    }

    fn resolve_internal_failure(&mut self, reason: String) {
        for sender in self.senders.drain(..) {
            let _ = sender.send(Err(MobError::Internal(reason.clone())));
        }
    }
}

#[cfg(feature = "runtime-adapter")]
impl Drop for PendingMemberTurnLlmIdentityApplication {
    fn drop(&mut self) {
        for sender in self.senders.drain(..) {
            let _ = sender.send(Err(MobError::Internal(
                "member turn ended before reaching the LLM identity application boundary"
                    .to_string(),
            )));
        }
    }
}

#[cfg(feature = "runtime-adapter")]
struct RuntimeQueuedTurnRegistration {
    owner: Arc<RuntimeQueuedTurnOwner>,
    input_id: InputId,
    owns_context: bool,
}

#[cfg(feature = "runtime-adapter")]
impl RuntimeQueuedTurnRegistration {
    fn rekey(&mut self, input_id: InputId) -> bool {
        match self
            .owner
            .rekey_turn_context(&self.input_id, input_id.clone())
        {
            RuntimeTurnContextRekey::Moved => {
                self.input_id = input_id;
                true
            }
            RuntimeTurnContextRekey::DestinationAlreadyOwned | RuntimeTurnContextRekey::Missing => {
                // The attempted request no longer owns a map entry. In the
                // dedup case the destination belongs to another reservation;
                // Drop must never remove that request's canonical context.
                self.owns_context = false;
                false
            }
        }
    }

    fn resolve_without_execution(&mut self, identity: Option<meerkat_core::SessionLlmIdentity>) {
        if !self.owns_context {
            return;
        }
        if let Some(mut context) = self.owner.discard_turn_context(&self.input_id) {
            context.llm_identity_application.resolve_success(identity);
        }
        self.owns_context = false;
    }

    fn resolve_llm_identity_at_admission(
        &self,
        identity: Option<meerkat_core::SessionLlmIdentity>,
    ) {
        if self.owns_context {
            self.owner
                .resolve_turn_llm_identity(&self.input_id, identity);
        }
    }
}

#[cfg(feature = "runtime-adapter")]
impl Drop for RuntimeQueuedTurnRegistration {
    fn drop(&mut self) {
        if self.owns_context {
            let _ = self.owner.discard_turn_context(&self.input_id);
        }
    }
}

#[cfg(feature = "runtime-adapter")]
enum RuntimeTurnContextRekey {
    Moved,
    DestinationAlreadyOwned,
    Missing,
}

#[cfg(feature = "runtime-adapter")]
impl RuntimeQueuedTurnOwner {
    fn new(attachment_witness: RuntimeExecutorAttachmentWitness) -> Self {
        Self {
            state: StdMutex::new(RuntimeQueuedTurnOwnerState {
                attachment_witness,
                queue: RuntimeSessionQueue::default(),
            }),
        }
    }

    fn is_owned_by(&self, witness: &RuntimeExecutorAttachmentWitness) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .attachment_witness
            == *witness
    }

    fn register_turn_context(
        self: &Arc<Self>,
        attachment: &RuntimeExecutorAttachmentWitness,
        input_id: InputId,
        event_tx: Option<TurnEventTx>,
        llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
    ) -> Result<RuntimeQueuedTurnRegistration, MobError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.attachment_witness != *attachment {
            return Err(MobError::Internal(format!(
                "runtime turn context owner changed before registration for input '{input_id}'"
            )));
        }
        if state.queue.entries.contains_key(&input_id) {
            return Err(MobError::Internal(format!(
                "runtime turn context for input '{input_id}' is already registered"
            )));
        }
        state.queue.entries.insert(
            input_id.clone(),
            QueuedTurnContext::new(event_tx, llm_identity_applied_tx),
        );
        Ok(RuntimeQueuedTurnRegistration {
            owner: Arc::clone(self),
            input_id,
            owns_context: true,
        })
    }

    fn take_turn_context_for_inputs(
        &self,
        attachment: &RuntimeExecutorAttachmentWitness,
        contributing_input_ids: &[InputId],
    ) -> Option<QueuedTurnContext> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.attachment_witness != *attachment {
            return None;
        }
        let mut selected: Option<QueuedTurnContext> = None;
        for input_id in contributing_input_ids {
            if let Some(context) = state.queue.entries.remove(input_id) {
                // A primitive may contribute multiple independently observed
                // inputs. Preserve every request-scoped observer in canonical
                // contribution order; last-wins selection would silently
                // abandon all earlier event streams and identity waiters.
                match selected.as_mut() {
                    Some(selected) => selected.append(context),
                    None => selected = Some(context),
                }
            }
        }
        selected
    }

    fn rekey_turn_context(
        &self,
        from_input_id: &InputId,
        to_input_id: InputId,
    ) -> RuntimeTurnContextRekey {
        if from_input_id == &to_input_id {
            return RuntimeTurnContextRekey::Moved;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.queue.entries.contains_key(&to_input_id) {
            state.queue.entries.remove(from_input_id);
            return RuntimeTurnContextRekey::DestinationAlreadyOwned;
        }
        let Some(context) = state.queue.entries.remove(from_input_id) else {
            return RuntimeTurnContextRekey::Missing;
        };
        state.queue.entries.insert(to_input_id, context);
        RuntimeTurnContextRekey::Moved
    }

    fn discard_turn_context(&self, input_id: &InputId) -> Option<QueuedTurnContext> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .queue
            .entries
            .remove(input_id)
    }

    fn resolve_turn_llm_identity(
        &self,
        input_id: &InputId,
        identity: Option<meerkat_core::SessionLlmIdentity>,
    ) {
        if let Some(context) = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .queue
            .entries
            .get_mut(input_id)
        {
            context.llm_identity_application.resolve_success(identity);
        }
    }

    fn clear_if_owned_by(&self, attachment: &RuntimeExecutorAttachmentWitness) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.attachment_witness != *attachment {
            return false;
        }
        state.queue.entries.clear();
        true
    }
}

#[cfg(feature = "runtime-adapter")]
impl RuntimeSessionState {
    pub(super) fn for_attachment(
        witness: RuntimeExecutorAttachmentWitness,
        actor_witness: meerkat_session::LiveSessionActorWitness,
    ) -> Self {
        let queued_turn_owner = Arc::new(RuntimeQueuedTurnOwner::new(witness.clone()));
        Self {
            attachment_witness: witness,
            actor_witness: StdMutex::new(Some(actor_witness)),
            attachment_phase: std::sync::atomic::AtomicU8::new(RUNTIME_ATTACHMENT_PENDING),
            operation_gate: Arc::new(Mutex::new(())),
            queued_turn_owner,
        }
    }

    /// Test-only fail-closed sidecar for a synthetic service that has no
    /// actor-incarnation registry. Production attachment paths must always use
    /// `for_attachment` with a service-minted exact witness.
    #[cfg(test)]
    pub(super) fn for_attachment_without_actor_witness(
        witness: RuntimeExecutorAttachmentWitness,
    ) -> Self {
        let queued_turn_owner = Arc::new(RuntimeQueuedTurnOwner::new(witness.clone()));
        Self {
            attachment_witness: witness,
            actor_witness: StdMutex::new(None),
            attachment_phase: std::sync::atomic::AtomicU8::new(RUNTIME_ATTACHMENT_PENDING),
            operation_gate: Arc::new(Mutex::new(())),
            queued_turn_owner,
        }
    }

    pub(super) fn witness(&self) -> &RuntimeExecutorAttachmentWitness {
        &self.attachment_witness
    }

    pub(super) fn actor_witness(&self) -> Option<meerkat_session::LiveSessionActorWitness> {
        self.actor_witness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Publish the exact replacement actor for this still-current attachment.
    /// Actor-only recovery retains the machine mutation gate while calling
    /// this, so the attachment cannot be replaced between validation and the
    /// synchronous witness swap. The executor cleanup handle owns this same
    /// state object and therefore observes the replacement actor thereafter.
    fn replace_actor_witness(
        &self,
        attachment: &RuntimeExecutorAttachmentWitness,
        actor_witness: meerkat_session::LiveSessionActorWitness,
    ) -> Result<(), MobError> {
        if !self.attachment_matches(attachment) {
            return Err(MobError::Internal(format!(
                "actor recovery for '{}' lost its exact executor attachment",
                attachment.session_id()
            )));
        }
        if actor_witness.session_id() != attachment.session_id() {
            return Err(MobError::Internal(format!(
                "actor recovery for '{}' produced actor witness for '{}'",
                attachment.session_id(),
                actor_witness.session_id()
            )));
        }
        *self
            .actor_witness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(actor_witness);
        Ok(())
    }

    fn attachment_matches(&self, witness: &RuntimeExecutorAttachmentWitness) -> bool {
        &self.attachment_witness == witness
    }

    pub(super) fn attachment_is_pending(&self, witness: &RuntimeExecutorAttachmentWitness) -> bool {
        self.attachment_matches(witness)
            && self
                .attachment_phase
                .load(std::sync::atomic::Ordering::Acquire)
                == RUNTIME_ATTACHMENT_PENDING
    }

    fn activate_attachment(&self, witness: &RuntimeExecutorAttachmentWitness) -> bool {
        self.attachment_matches(witness)
            && self
                .attachment_phase
                .compare_exchange(
                    RUNTIME_ATTACHMENT_PENDING,
                    RUNTIME_ATTACHMENT_ACTIVE,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_ok()
    }

    pub(super) fn stage_attachment_activation(
        self: &Arc<Self>,
        witness: &RuntimeExecutorAttachmentWitness,
    ) -> Option<StagedRuntimeSessionActivation> {
        self.activate_attachment(witness)
            .then(|| StagedRuntimeSessionActivation {
                state: Arc::clone(self),
                witness: witness.clone(),
                armed: true,
            })
    }

    pub(super) fn attachment_is_active(&self, witness: &RuntimeExecutorAttachmentWitness) -> bool {
        self.attachment_matches(witness)
            && self
                .attachment_phase
                .load(std::sync::atomic::Ordering::Acquire)
                == RUNTIME_ATTACHMENT_ACTIVE
    }

    pub(super) fn mark_cleanup_scheduled(&self) -> bool {
        loop {
            let phase = self
                .attachment_phase
                .load(std::sync::atomic::Ordering::Acquire);
            if phase >= RUNTIME_ATTACHMENT_RETIRING {
                return false;
            }
            if self
                .attachment_phase
                .compare_exchange(
                    phase,
                    RUNTIME_ATTACHMENT_RETIRING,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    /// Fail closed while exact machine retirement is owned by an asynchronous
    /// saga whose outcome has not yet been observed. This is deliberately
    /// distinct from `Retiring`: the actor, sidecar, and queued request
    /// contexts remain intact until the machine has durably entered its drain
    /// path and invokes canonical post-stop cleanup.
    fn mark_retirement_uncertain(&self) -> bool {
        loop {
            let phase = self
                .attachment_phase
                .load(std::sync::atomic::Ordering::Acquire);
            if phase >= RUNTIME_ATTACHMENT_RETIREMENT_UNCERTAIN {
                return false;
            }
            if self
                .attachment_phase
                .compare_exchange(
                    phase,
                    RUNTIME_ATTACHMENT_RETIREMENT_UNCERTAIN,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn retirement_is_uncertain(&self) -> bool {
        self.attachment_phase
            .load(std::sync::atomic::Ordering::Acquire)
            == RUNTIME_ATTACHMENT_RETIREMENT_UNCERTAIN
    }

    pub(super) fn cleanup_scheduled(&self) -> bool {
        self.attachment_phase
            .load(std::sync::atomic::Ordering::Acquire)
            >= RUNTIME_ATTACHMENT_RETIRING
    }

    fn mark_retired(&self) {
        self.attachment_phase.store(
            RUNTIME_ATTACHMENT_RETIRED,
            std::sync::atomic::Ordering::Release,
        );
    }

    async fn operation_guard(&self) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.operation_gate).lock_owned().await
    }

    async fn exact_operation_guard(
        &self,
        adapter: &Arc<MeerkatMachine>,
        session_id: &SessionId,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, MobError> {
        let guard = self.operation_guard().await;
        if !self.attachment_is_active(self.witness()) {
            return Err(MobError::Internal(format!(
                "runtime attachment for session '{session_id}' is not active"
            )));
        }
        let current = adapter
            .current_executor_attachment_witness(session_id)
            .await;
        if current.as_ref() != Some(self.witness()) {
            return Err(MobError::Internal(format!(
                "runtime attachment changed before input admission for session '{session_id}'"
            )));
        }
        Ok(guard)
    }

    fn register_turn_context(
        self: &Arc<Self>,
        input_id: InputId,
        event_tx: Option<TurnEventTx>,
        llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
    ) -> Result<Option<RuntimeQueuedTurnRegistration>, MobError> {
        if event_tx.is_none() && llm_identity_applied_tx.is_none() {
            return Ok(None);
        }
        self.queued_turn_owner
            .register_turn_context(self.witness(), input_id, event_tx, llm_identity_applied_tx)
            .map(Some)
    }

    async fn take_turn_context_for_inputs(
        &self,
        contributing_input_ids: &[InputId],
    ) -> Option<QueuedTurnContext> {
        self.queued_turn_owner
            .take_turn_context_for_inputs(self.witness(), contributing_input_ids)
    }

    pub(super) async fn clear_queued_turns(&self) {
        let _ = self.queued_turn_owner.clear_if_owned_by(self.witness());
    }
}

/// Reversible exact-sidecar activation held only through the machine's final
/// retained publication. If the callback panics or the runtime-loop serving
/// release fails, Drop restores Pending while M is still retained. A state
/// already moved to Retiring remains fail-closed instead of being revived.
#[cfg(feature = "runtime-adapter")]
pub(super) struct StagedRuntimeSessionActivation {
    state: Arc<RuntimeSessionState>,
    witness: RuntimeExecutorAttachmentWitness,
    armed: bool,
}

#[cfg(feature = "runtime-adapter")]
impl StagedRuntimeSessionActivation {
    pub(super) fn finalize(mut self) {
        self.armed = false;
    }
}

#[cfg(feature = "runtime-adapter")]
impl Drop for StagedRuntimeSessionActivation {
    fn drop(&mut self) {
        if !self.armed || !self.state.attachment_matches(&self.witness) {
            return;
        }
        let _ = self.state.attachment_phase.compare_exchange(
            RUNTIME_ATTACHMENT_ACTIVE,
            RUNTIME_ATTACHMENT_PENDING,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
    }
}

#[cfg(feature = "runtime-adapter")]
pub(super) struct RuntimeTurnFinalizationBoundaryLease {
    session_id: SessionId,
    _guard: Box<dyn CoreExecutorTurnFinalizationGuard>,
}

#[cfg(feature = "runtime-adapter")]
impl RuntimeTurnFinalizationBoundaryLease {
    pub(super) async fn acquire(
        session_service: &Arc<dyn MobSessionService>,
        session_id: &SessionId,
    ) -> Result<Self, MobError> {
        let guard = session_service
            .acquire_runtime_turn_finalization_guard(session_id)
            .await?;
        Ok(Self {
            session_id: session_id.clone(),
            _guard: guard,
        })
    }

    fn require_session(&self, session_id: &SessionId) -> Result<(), MobError> {
        if &self.session_id == session_id {
            Ok(())
        } else {
            Err(MobError::Internal(format!(
                "runtime turn boundary for '{}' cannot authorize materialization of '{session_id}'",
                self.session_id
            )))
        }
    }
}

/// Reconstruct the service-owned actor for one already-serving exact executor
/// attachment. The complete create -> witness publication -> machine commit
/// runs in the process-owned cleanup runtime while retaining B and exact M, so
/// caller cancellation may lose the response but cannot leave a half-published
/// actor or replace the executor attachment.
#[cfg(feature = "runtime-adapter")]
async fn create_attached_session_actor_recovery_owned(
    session_id: SessionId,
    session_service: Arc<dyn MobSessionService>,
    mut prepared: PreparedAttachedSessionActorRecovery,
    boundary: RuntimeTurnFinalizationBoundaryLease,
    state: Arc<RuntimeSessionState>,
    actor_witness_slot: meerkat_session::LiveSessionActorWitnessSlot,
    req: meerkat_core::service::CreateSessionRequest,
) -> Result<meerkat_core::RunResult, MobError> {
    boundary.require_session(&session_id)?;
    if prepared.witness().session_id() != &session_id || state.witness() != prepared.witness() {
        return Err(MobError::Internal(format!(
            "actor-only recovery for '{session_id}' lost its exact attachment tuple"
        )));
    }
    let cleanup_spawner = meerkat_runtime::RuntimeCleanupTaskSpawner::acquire()
        .map_err(|error| MobError::Internal(error.to_string()))?;
    let wait_session_id = session_id.clone();
    let (result_tx, result_rx) = oneshot::channel();
    cleanup_spawner.spawn_detached(async move {
        let _boundary = boundary;
        let create_result = session_service
            .create_session_with_actor_witness_under_runtime_turn_boundary(
                req,
                &actor_witness_slot,
            )
            .await;
        let result = match create_result {
            Ok(created) if created.session_id != session_id => {
                let cleanup = match actor_witness_slot.witness() {
                    Some(actor_witness) => session_service
                        .discard_live_session_actor_under_runtime_turn_boundary(&actor_witness)
                        .await
                        .map(|_| ()),
                    None => session_service
                        .discard_live_session_under_runtime_turn_boundary(&session_id)
                        .await,
                };
                Err(MobError::Internal(match cleanup {
                    Ok(()) | Err(SessionError::NotFound { .. }) => format!(
                        "actor-only recovery for '{session_id}' created unexpected session '{}'",
                        created.session_id
                    ),
                    Err(cleanup_error) => format!(
                        "actor-only recovery for '{session_id}' created unexpected session '{}'; actor cleanup also failed: {cleanup_error}",
                        created.session_id
                    ),
                }))
            }
            Ok(created) => {
                let actor_witness = match actor_witness_slot.witness() {
                    Some(actor_witness) => actor_witness,
                    None => {
                        let cleanup = session_service
                            .discard_live_session_under_runtime_turn_boundary(&session_id)
                            .await;
                        let error = MobError::Internal(match cleanup {
                            Ok(()) | Err(SessionError::NotFound { .. }) => format!(
                                "actor-only recovery for '{session_id}' returned without an exact actor witness"
                            ),
                            Err(cleanup_error) => format!(
                                "actor-only recovery for '{session_id}' returned without an exact actor witness; actor cleanup also failed: {cleanup_error}"
                            ),
                        });
                        let _ = result_tx.send(Err(error));
                        return;
                    }
                };
                let publication = state
                    .replace_actor_witness(prepared.witness(), actor_witness.clone())
                    .and_then(|()| {
                        prepared
                            .commit_actor()
                            .map_err(|error| MobError::Internal(error.to_string()))
                    });
                match publication {
                    Ok(()) => Ok(created),
                    Err(error) => {
                        let cleanup = session_service
                            .discard_live_session_actor_under_runtime_turn_boundary(&actor_witness)
                            .await
                            .map(|_| ());
                        Err(MobError::Internal(match cleanup {
                            Ok(()) | Err(SessionError::NotFound { .. }) => error.to_string(),
                            Err(cleanup_error) => format!(
                                "{error}; exact recovered-actor cleanup also failed: {cleanup_error}"
                            ),
                        }))
                    }
                }
            }
            Err(error) => Err(MobError::from(error)),
        };
        // A lost response after successful commit is an ambiguous distributed
        // outcome. The actor and attachment remain one valid resumable
        // incarnation; callers resolve the outcome through discovery/retry.
        let _ = result_tx.send(result);
    });
    result_rx.await.map_err(|error| {
        MobError::Internal(format!(
            "owned actor-only recovery for '{wait_session_id}' ended without a result: {error}"
        ))
    })?
}

/// Cancellation-safe owner of the service actor and exact prepared machine
/// claim between actor creation and the final executor/operation commit.
///
/// The guard owns B as well as Prepared. Dropping the caller future transfers
/// both into an owned cleanup task, so cancellation cannot strand a live actor
/// or a non-serving attachment. A durably-created session document is not
/// compensation-owned by this transaction and remains discoverable/resumable.
#[cfg(feature = "runtime-adapter")]
pub(super) struct PreparedServiceActorTransaction {
    session_id: SessionId,
    session_service: Arc<dyn MobSessionService>,
    prepared: Option<PreparedSessionMaterialization>,
    boundary: Option<RuntimeTurnFinalizationBoundaryLease>,
    actor_witness_slot: meerkat_session::LiveSessionActorWitnessSlot,
    cleanup_spawner: meerkat_runtime::RuntimeCleanupTaskSpawner,
    // Exact serving attachment published by the local provision path but not
    // yet covered by a returned spawn receipt. Keeping this in the same RAII
    // owner as B makes cancellation after oneshot send but before receive
    // retire the exact attachment before actor cleanup.
    committed_runtime_attachment: Option<(Arc<MeerkatMachine>, Arc<RuntimeSessionState>)>,
    armed: bool,
}

#[cfg(feature = "runtime-adapter")]
impl PreparedServiceActorTransaction {
    pub(super) fn new(
        session_id: SessionId,
        session_service: Arc<dyn MobSessionService>,
        prepared: PreparedSessionMaterialization,
        boundary: RuntimeTurnFinalizationBoundaryLease,
        actor_witness_slot: meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<Self, MobError> {
        boundary.require_session(&session_id)?;
        if prepared.session_id() != &session_id {
            return Err(MobError::Internal(format!(
                "prepared materialization for '{}' cannot own actor transaction for '{session_id}'",
                prepared.session_id()
            )));
        }
        let cleanup_spawner = prepared.cleanup_task_spawner();
        Ok(Self {
            session_id,
            session_service,
            prepared: Some(prepared),
            boundary: Some(boundary),
            actor_witness_slot,
            cleanup_spawner,
            committed_runtime_attachment: None,
            armed: true,
        })
    }

    fn prepared(&self) -> Result<&PreparedSessionMaterialization, MobError> {
        self.prepared.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "actor transaction for '{}' lost its prepared materialization",
                self.session_id
            ))
        })
    }

    fn prepared_mut(&mut self) -> Result<&mut PreparedSessionMaterialization, MobError> {
        self.prepared.as_mut().ok_or_else(|| {
            MobError::Internal(format!(
                "actor transaction for '{}' lost its prepared materialization",
                self.session_id
            ))
        })
    }

    fn boundary(&self) -> Result<&RuntimeTurnFinalizationBoundaryLease, MobError> {
        self.boundary.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "actor transaction for '{}' lost its turn-finalization boundary",
                self.session_id
            ))
        })
    }

    pub(super) fn prepared_and_boundary_mut(
        &mut self,
    ) -> Result<
        (
            &mut PreparedSessionMaterialization,
            &mut RuntimeTurnFinalizationBoundaryLease,
        ),
        MobError,
    > {
        let prepared = self.prepared.as_mut().ok_or_else(|| {
            MobError::Internal(format!(
                "actor transaction for '{}' lost its prepared materialization",
                self.session_id
            ))
        })?;
        let boundary = self.boundary.as_mut().ok_or_else(|| {
            MobError::Internal(format!(
                "actor transaction for '{}' lost its turn-finalization boundary",
                self.session_id
            ))
        })?;
        Ok((prepared, boundary))
    }

    pub(super) fn boundary_slot_mut(
        &mut self,
    ) -> Result<&mut Option<RuntimeTurnFinalizationBoundaryLease>, MobError> {
        if self.boundary.is_none() {
            return Err(MobError::Internal(format!(
                "actor transaction for '{}' lost its turn-finalization boundary",
                self.session_id
            )));
        }
        Ok(&mut self.boundary)
    }

    pub(super) fn actor_witness_slot(&self) -> &meerkat_session::LiveSessionActorWitnessSlot {
        &self.actor_witness_slot
    }

    pub(super) fn actor_witness(
        &self,
    ) -> Result<meerkat_session::LiveSessionActorWitness, MobError> {
        self.actor_witness_slot.witness().ok_or_else(|| {
            MobError::Internal(format!(
                "actor transaction for '{}' reached attachment without an exact live actor witness",
                self.session_id
            ))
        })
    }

    /// Run the complete service create/persist tail in the machine cleanup
    /// runtime while that task owns Prepared+B+the actor witness slot. If the
    /// caller disappears, the owned task finishes every possibly detached
    /// durable write before exact volatile cleanup. A completed durable create
    /// remains discoverable and may be explicitly resumed.
    pub(super) async fn create_owned(
        self,
        req: meerkat_core::service::CreateSessionRequest,
        archived_resume_authorization: Option<
            meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        >,
    ) -> Result<(meerkat_core::RunResult, Self), MobError> {
        let cleanup_spawner = self.cleanup_spawner.clone();
        let session_id = self.session_id.clone();
        let wait_session_id = session_id.clone();
        let (result_tx, result_rx) = oneshot::channel();
        cleanup_spawner.spawn_detached(async move {
            let transaction = self;
            let create_result = match archived_resume_authorization {
                Some(authorization) => transaction
                    .session_service
                    .create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
                        req,
                        authorization,
                        transaction.actor_witness_slot(),
                    )
                    .await,
                None => transaction
                    .session_service
                    .create_session_with_actor_witness_under_runtime_turn_boundary(
                        req,
                        transaction.actor_witness_slot(),
                    )
                    .await,
            };
            match create_result {
                Ok(result) => {
                    if let Err(payload) = result_tx.send(Ok((result, transaction))) {
                        let Ok((_result, transaction)) = payload else {
                            tracing::error!(
                                %session_id,
                                "successful actor create sender returned an impossible error payload"
                            );
                            return;
                        };
                        if let Err(error) = transaction.abort().await {
                            tracing::error!(
                                %session_id,
                                %error,
                                "detached successful actor create cleanup failed"
                            );
                        }
                    }
                }
                Err(create_error) => {
                    let cleanup = transaction.abort().await;
                    let error = match cleanup {
                        // Preserve the service's typed build failure once
                        // exact volatile cleanup is proven. Callers can then
                        // project the class without parsing display text.
                        Ok(()) => MobError::SessionError(create_error),
                        Err(cleanup_error) => {
                            // Build diagnostics may contain host-local command
                            // stderr or backend detail. Keep both diagnostics
                            // in the owning host log; callers get only a
                            // sanitized uncertainty message.
                            tracing::error!(
                                %session_id,
                                build_detail = %create_error,
                                cleanup_detail = %cleanup_error,
                                "actor create and exact materialization cleanup both failed"
                            );
                            MobError::Internal(
                                "agent build failed and exact actor/materialization cleanup also failed; inspect host logs"
                                    .to_string(),
                            )
                        }
                    };
                    let _ = result_tx.send(Err(error));
                }
            }
        });
        result_rx.await.map_err(|error| {
            MobError::Internal(format!(
                "owned actor create for '{wait_session_id}' ended without a result: {error}"
            ))
        })?
    }

    /// Run the complete local prepare -> attach -> operation publication in a
    /// process-owned task that retains Prepared+B and every intermediate M
    /// lease across startup recovery and the durable lifecycle write.
    ///
    /// The returned transaction remains armed with exact committed-attachment
    /// rollback until the caller publishes the spawn receipt via `commit`.
    /// Thus cancellation both before and after oneshot delivery retires the
    /// serving attachment before discarding its exact actor.
    pub(super) async fn prepare_and_commit_runtime_attachment_and_operation_owned(
        self,
        adapter: Arc<MeerkatMachine>,
        workgraph_service: Option<meerkat::WorkGraphService>,
        runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
        missing_live_revival: bool,
        prepared_operation: super::ops_adapter::PreparedMemberProvisionOperation,
    ) -> Result<
        (
            Arc<RuntimeSessionState>,
            super::ops_adapter::CommittedMemberProvisionOperation,
            Self,
        ),
        MobError,
    > {
        let cleanup_spawner = self.cleanup_spawner.clone();
        let session_id = self.session_id.clone();
        let wait_session_id = session_id.clone();
        let (result_tx, result_rx) = oneshot::channel();
        cleanup_spawner.spawn_detached(async move {
            let mut transaction = self;
            let commit_result = async {
                let actor_witness = transaction.actor_witness()?;
                let session_service = Arc::clone(&transaction.session_service);
                // B has been held since before machine preparation, so an old
                // attachment's cleanup cannot mutate this slot while we select
                // the exact A witness to replace. The replacement receives a
                // fresh attachment-local queue; A's queued work never moves.
                let replaced_state = if missing_live_revival {
                    let state = runtime_sessions.read().await.get(&session_id).cloned();
                    if let Some(state) = state.as_ref()
                        && (!state.attachment_is_active(state.witness())
                            || state.cleanup_scheduled()
                            || !state.queued_turn_owner.is_owned_by(state.witness()))
                    {
                        return Err(MobError::Internal(format!(
                            "missing-live replacement for '{session_id}' found a stale sidecar that no longer owns active queue plumbing"
                        )));
                    }
                    state
                } else {
                    None
                };
                let (candidate, pending) = {
                    let (prepared, boundary) = transaction.prepared_and_boundary_mut()?;
                    prepare_prepared_runtime_session_state(
                        prepared,
                        actor_witness,
                        session_service,
                        Arc::clone(&adapter),
                        workgraph_service,
                        Arc::clone(&runtime_sessions),
                        boundary,
                    )
                    .await?
                };
                let committed_operation_slot = Arc::new(StdMutex::new(None));
                let callback_slot = Arc::clone(&committed_operation_slot);
                let committed_state = match transaction.boundary.as_mut() {
                    Some(boundary) => {
                        commit_pending_runtime_session_state(
                            &adapter,
                            &session_id,
                            &runtime_sessions,
                            candidate,
                            replaced_state.as_ref(),
                            pending,
                            boundary,
                            move |_| {
                                let committed = prepared_operation.commit()?;
                                *callback_slot
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                    Some(committed);
                                Ok(())
                            },
                        )
                        .await?
                    }
                    None => {
                        return Err(MobError::Internal(format!(
                            "owned local attachment commit for '{session_id}' lost B"
                        )));
                    }
                };
                // Record rollback authority immediately after the attachment
                // becomes serving. Even an impossible callback/result
                // mismatch must retire this exact incarnation.
                transaction.committed_runtime_attachment =
                    Some((Arc::clone(&adapter), Arc::clone(&committed_state)));
                let committed_operation = committed_operation_slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "attachment commit for '{session_id}' omitted its exact operation commit"
                        ))
                    })?;
                Ok::<_, MobError>((committed_state, committed_operation))
            }
            .await;

            match commit_result {
                Ok((committed_state, committed_operation)) => {
                    if let Err(payload) = result_tx.send(Ok((
                        committed_state,
                        committed_operation,
                        transaction,
                    ))) {
                        let Ok((_state, committed_operation, transaction)) = payload else {
                            tracing::error!(
                                %session_id,
                                "successful local attachment sender returned an impossible error payload"
                            );
                            return;
                        };
                        drop(committed_operation);
                        if let Err(error) = transaction.abort().await {
                            tracing::error!(
                                %session_id,
                                %error,
                                "detached successful local attachment cleanup failed"
                            );
                        }
                    }
                }
                Err(commit_error) => {
                    // The inner transaction future has already dropped any
                    // callback-staged operation before cleanup releases B.
                    let cleanup = transaction.abort().await;
                    let error = match cleanup {
                        Ok(()) => commit_error,
                        Err(cleanup_error) => MobError::Internal(format!(
                            "{commit_error}; exact actor/materialization cleanup also failed: {cleanup_error}"
                        )),
                    };
                    let _ = result_tx.send(Err(error));
                }
            }
        });
        result_rx.await.map_err(|error| {
            MobError::Internal(format!(
                "owned local attachment transaction for '{wait_session_id}' ended without a result: {error}"
            ))
        })?
    }

    /// Consume the full host actor transaction into one process-owned
    /// prepare -> attach -> retained-publication transfer.
    ///
    /// B must never remain in an ambient caller while a Pending attachment or
    /// retained M lease can drop into ordinary post-stop cleanup. Keeping the
    /// transaction and every intermediate attachment lease in this one task
    /// closes that cancellation window; only the combined B+M publication
    /// lease crosses back to the host actor.
    pub(super) async fn prepare_host_runtime_publication_owned(
        self,
        adapter: Arc<MeerkatMachine>,
        runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    ) -> Result<CommittedRuntimeSessionPublicationLease, MobError> {
        let cleanup_spawner = self.cleanup_spawner.clone();
        let session_id = self.session_id.clone();
        let wait_session_id = session_id.clone();
        let (result_tx, result_rx) = oneshot::channel();
        cleanup_spawner.spawn_detached(async move {
            let mut transaction = self;
            let prepare_result = async {
                let actor_witness = transaction.actor_witness()?;
                let session_service = Arc::clone(&transaction.session_service);
                let (candidate, pending) = {
                    let (prepared, boundary) = transaction.prepared_and_boundary_mut()?;
                    prepare_prepared_runtime_session_state(
                        prepared,
                        actor_witness.clone(),
                        session_service,
                        Arc::clone(&adapter),
                        None,
                        Arc::clone(&runtime_sessions),
                        boundary,
                    )
                    .await?
                };

                if let Err(error) = adapter
                    .run_executor_attach_post_ensure_test_hook(&session_id)
                    .await
                {
                    let cleanup = pending
                        .abort_under_runtime_turn_finalization_boundary()
                        .await;
                    return Err(MobError::Internal(match cleanup {
                        Ok(()) => format!("pre-commit attachment hook failed: {error}"),
                        Err(cleanup_error) => format!(
                            "pre-commit attachment hook failed: {error}; exact pending-attachment cleanup failed: {cleanup_error}"
                        ),
                    }));
                }

                let publication = commit_pending_runtime_session_state_for_publication(
                    &adapter,
                    &session_id,
                    actor_witness,
                    &runtime_sessions,
                    candidate,
                    pending,
                    transaction.boundary_slot_mut()?,
                )
                .await?;
                transaction.commit();
                Ok::<_, MobError>(publication)
            }
            .await;

            match prepare_result {
                Ok(publication) => {
                    // Receiver loss drops the combined B+M lease in this
                    // process-owned task, which starts exact cleanup without
                    // ever splitting the two authorities across tasks.
                    let _ = result_tx.send(Ok(publication));
                }
                Err(error) => {
                    let cleanup = transaction.abort().await;
                    let error = match cleanup {
                        Ok(()) => error,
                        Err(cleanup_error) => MobError::Internal(format!(
                            "{error}; exact actor/materialization cleanup also failed: {cleanup_error}"
                        )),
                    };
                    let _ = result_tx.send(Err(error));
                }
            }
        });
        result_rx.await.map_err(|error| {
            MobError::Internal(format!(
                "owned host attachment transfer for '{wait_session_id}' ended without a result: {error}"
            ))
        })?
    }

    async fn cleanup_owned_parts(
        session_id: SessionId,
        session_service: Arc<dyn MobSessionService>,
        mut prepared: Option<PreparedSessionMaterialization>,
        boundary: Option<RuntimeTurnFinalizationBoundaryLease>,
        actor_witness_slot: meerkat_session::LiveSessionActorWitnessSlot,
        committed_runtime_attachment: Option<(Arc<MeerkatMachine>, Arc<RuntimeSessionState>)>,
    ) -> Result<(), MobError> {
        let mut boundary = Some(boundary.ok_or_else(|| {
            MobError::Internal(format!(
                "actor cleanup for '{session_id}' lost its turn-finalization boundary"
            ))
        })?);
        boundary
            .as_ref()
            .expect("boundary inserted above")
            .require_session(&session_id)?;
        let mut errors = Vec::new();
        if let Some((adapter, state)) = committed_runtime_attachment {
            let attachment_witness = state.witness().clone();
            match adapter
                .prepare_executor_attachment_retirement_under_runtime_turn_boundary(
                    &attachment_witness,
                )
                .await
            {
                Ok(Some(retirement)) => {
                    // From this point the normal machine-owned unregister
                    // saga owns exact M. Keep the complete attachment-local
                    // assembly available, but fail admission closed until the
                    // saga has durably entered Draining, stopped the executor,
                    // and invoked canonical actor/sidecar cleanup.
                    let completion = retirement.commit().map_err(|error| {
                        state.mark_retirement_uncertain();
                        MobError::Internal(format!(
                            "actor transaction cleanup for '{session_id}' could not transfer exact attachment retirement: {error}"
                        ))
                    })?;
                    state.mark_retirement_uncertain();
                    drop(boundary.take());
                    return match completion.wait().await {
                        Ok(true) => Ok(()),
                        Ok(false) => Err(MobError::Internal(format!(
                            "actor transaction cleanup for '{session_id}' lost its exact attachment after retirement transfer"
                        ))),
                        Err(error) => Err(MobError::Internal(format!(
                            "actor transaction cleanup for '{session_id}' left exact attachment retirement uncertain: {error}"
                        ))),
                    };
                }
                // A terminal runtime may already have completed the machine's
                // exact unregister saga before cancellation cleanup arrives.
                // If it is still present but Draining, join that exact saga
                // after releasing B instead of bypassing stop terminalization.
                Ok(None) if adapter.contains_session(&session_id).await => {
                    state.mark_retirement_uncertain();
                    drop(boundary.take());
                    return match adapter
                        .unregister_executor_attachment_if_current(&attachment_witness)
                        .await
                    {
                        Ok(true) => Ok(()),
                        Ok(false) => Err(MobError::Internal(format!(
                            "actor transaction cleanup for '{session_id}' could not join its exact in-progress retirement"
                        ))),
                        Err(error) => Err(MobError::Internal(format!(
                            "actor transaction cleanup for '{session_id}' left exact in-progress retirement uncertain: {error}"
                        ))),
                    };
                }
                Ok(None) => {}
                Err(error) => {
                    state.mark_retirement_uncertain();
                    drop(boundary.take());
                    return Err(MobError::Internal(format!(
                        "actor transaction cleanup for '{session_id}' could not prepare exact attachment retirement; actor and sidecar were preserved fail-closed: {error}"
                    )));
                }
            }
        }
        if let Some(actor_witness) = actor_witness_slot.witness() {
            match session_service
                .discard_live_session_actor_under_runtime_turn_boundary(&actor_witness)
                .await
            {
                Ok(_) | Err(SessionError::NotFound { .. }) => {}
                Err(error) => errors.push(format!("exact live actor discard: {error}")),
            }
        }
        if let Some(prepared) = prepared.as_mut()
            && let Err(error) = prepared
                .rollback_now_under_turn_finalization_boundary()
                .await
        {
            errors.push(format!("prepared materialization rollback: {error}"));
        }
        drop(boundary.take());
        if errors.is_empty() {
            Ok(())
        } else {
            Err(MobError::Internal(format!(
                "actor transaction cleanup for '{session_id}' failed: {}",
                errors.join("; ")
            )))
        }
    }

    pub(super) async fn abort(mut self) -> Result<(), MobError> {
        let session_id = self.session_id.clone();
        let session_service = Arc::clone(&self.session_service);
        let prepared = self.prepared.take();
        let boundary = self.boundary.take();
        let actor_witness_slot = self.actor_witness_slot.clone();
        let committed_runtime_attachment = self.committed_runtime_attachment.take();
        let cleanup_spawner = self.cleanup_spawner.clone();
        let (result_tx, result_rx) = oneshot::channel();
        cleanup_spawner.spawn_detached(async move {
            let result = Self::cleanup_owned_parts(
                session_id,
                session_service,
                prepared,
                boundary,
                actor_witness_slot,
                committed_runtime_attachment,
            )
            .await;
            let _ = result_tx.send(result);
        });
        self.armed = false;
        result_rx.await.map_err(|error| {
            MobError::Internal(format!(
                "owned actor materialization abort ended without a result: {error}"
            ))
        })?
    }

    pub(super) fn commit(&mut self) {
        self.committed_runtime_attachment.take();
        self.armed = false;
    }
}

#[cfg(feature = "runtime-adapter")]
impl Drop for PreparedServiceActorTransaction {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let session_id = self.session_id.clone();
        let session_service = Arc::clone(&self.session_service);
        let prepared = self.prepared.take();
        let boundary = self.boundary.take();
        let actor_witness_slot = self.actor_witness_slot.clone();
        let committed_runtime_attachment = self.committed_runtime_attachment.take();
        self.cleanup_spawner.spawn_detached(async move {
            if let Err(error) = PreparedServiceActorTransaction::cleanup_owned_parts(
                session_id.clone(),
                session_service,
                prepared,
                boundary,
                actor_witness_slot,
                committed_runtime_attachment,
            )
            .await
            {
                tracing::error!(
                    %session_id,
                    %error,
                    "cancelled actor materialization cleanup failed"
                );
            }
        });
    }
}

#[cfg(feature = "runtime-adapter")]
fn same_runtime_attachment(
    current: &Arc<RuntimeSessionState>,
    expected: &Arc<RuntimeSessionState>,
) -> bool {
    current.witness() == expected.witness()
}

/// Reversible publication of pending sidecar B into the logical session slot.
/// The slot may be empty (ordinary attach) or owned by the exact stale
/// sidecar A selected by MissingLive preparation. B remains non-admitting
/// until the machine commit callback activates it; Drop restores A if any
/// later commit step fails.
#[cfg(feature = "runtime-adapter")]
struct StagedRuntimeSessionMapPublication<'a> {
    sessions: &'a mut HashMap<SessionId, Arc<RuntimeSessionState>>,
    session_id: SessionId,
    previous: Option<Arc<RuntimeSessionState>>,
    candidate: Arc<RuntimeSessionState>,
    armed: bool,
}

#[cfg(feature = "runtime-adapter")]
impl<'a> StagedRuntimeSessionMapPublication<'a> {
    fn validate_stage(
        sessions: &HashMap<SessionId, Arc<RuntimeSessionState>>,
        session_id: &SessionId,
        expected_previous: Option<&Arc<RuntimeSessionState>>,
    ) -> Result<(), MobError> {
        match (sessions.get(session_id), expected_previous) {
            (None, None) => {}
            (Some(current), Some(expected)) if Arc::ptr_eq(current, expected) => {}
            (Some(current), _) => {
                return Err(MobError::Internal(format!(
                    "runtime sidecar slot for session '{session_id}' is owned by unexpected attachment {:?}",
                    current.witness()
                )));
            }
            (None, Some(expected)) => {
                return Err(MobError::Internal(format!(
                    "runtime sidecar slot for session '{session_id}' lost expected attachment {:?} before replacement",
                    expected.witness()
                )));
            }
        }
        Ok(())
    }

    fn stage_validated(
        sessions: &'a mut HashMap<SessionId, Arc<RuntimeSessionState>>,
        session_id: &SessionId,
        candidate: Arc<RuntimeSessionState>,
    ) -> Self {
        let previous = sessions.insert(session_id.clone(), Arc::clone(&candidate));
        Self {
            sessions,
            session_id: session_id.clone(),
            previous,
            candidate,
            armed: true,
        }
    }

    fn finalize(mut self) {
        self.armed = false;
    }
}

#[cfg(feature = "runtime-adapter")]
impl Drop for StagedRuntimeSessionMapPublication<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if !self
            .sessions
            .get(&self.session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &self.candidate))
        {
            return;
        }
        match self.previous.take() {
            Some(previous) => {
                self.sessions.insert(self.session_id.clone(), previous);
            }
            None => {
                self.sessions.remove(&self.session_id);
            }
        }
    }
}

/// Exact host-side publication transaction retained after machine attachment
/// commit and consumed only when durable residency publication succeeds.
/// Abort owns volatile quiescence only: a durably-created session document is
/// preserved for discovery/resume after cancellation or an ambiguous response.
#[cfg(feature = "runtime-adapter")]
#[must_use = "committed runtime publication must be committed or aborted"]
pub(super) struct CommittedRuntimeSessionPublicationLease {
    session_id: SessionId,
    state: Arc<RuntimeSessionState>,
    cleanup_spawner: meerkat_runtime::RuntimeCleanupTaskSpawner,
    machine_lease: Option<meerkat_runtime::CommittedRuntimeExecutorAttachmentPublicationLease>,
    boundary: Option<RuntimeTurnFinalizationBoundaryLease>,
    armed: bool,
}

#[cfg(feature = "runtime-adapter")]
impl CommittedRuntimeSessionPublicationLease {
    async fn abort_after_publication_failure(self, original: String) -> MobError {
        match self.abort().await {
            Ok(()) => MobError::Internal(original),
            Err(cleanup_error) => MobError::Internal(format!(
                "{original}; exact host publication cleanup also failed: {cleanup_error}"
            )),
        }
    }

    async fn abort_owned_parts(
        session_id: SessionId,
        state: Arc<RuntimeSessionState>,
        machine_lease: Option<meerkat_runtime::CommittedRuntimeExecutorAttachmentPublicationLease>,
        boundary: Option<RuntimeTurnFinalizationBoundaryLease>,
    ) -> Result<(), MobError> {
        let boundary = boundary.ok_or_else(|| {
            MobError::Internal(format!(
                "committed publication abort for '{session_id}' lost B"
            ))
        })?;
        boundary.require_session(&session_id)?;
        let Some(machine_lease) = machine_lease else {
            state.mark_retirement_uncertain();
            drop(boundary);
            return Err(MobError::Internal(format!(
                "committed publication abort for '{session_id}' lost exact M authority; actor and sidecar were preserved fail-closed"
            )));
        };
        let completion = match machine_lease.begin_abort() {
            Ok(completion) => completion,
            Err(error) => {
                state.mark_retirement_uncertain();
                drop(boundary);
                return Err(MobError::Internal(format!(
                    "committed publication abort for '{session_id}' could not transfer exact attachment retirement; actor and sidecar were preserved fail-closed: {error}"
                )));
            }
        };
        // The exact M authority now belongs to canonical unregister. Keep the
        // attachment-local assembly intact and non-admitting while its result
        // is unknown; queued contexts are cleared only by post-stop cleanup.
        state.mark_retirement_uncertain();
        drop(boundary);
        match completion.wait().await {
            Ok(true) => Ok(()),
            Ok(false) => Err(MobError::Internal(format!(
                "committed publication abort for '{session_id}' lost its exact attachment after retirement transfer"
            ))),
            Err(error) => Err(MobError::Internal(format!(
                "committed publication abort for '{session_id}' left exact attachment retirement uncertain: {error}"
            ))),
        }
    }

    /// Boot-revival-only predecessor fence. The exact attachment remains
    /// pending and retains the machine mutation gate while the recovered
    /// driver's old request set is durably abandoned without publishing a
    /// replacement-generated terminal.
    pub(super) async fn abandon_recovered_predecessor_inputs(&mut self) -> Result<usize, MobError> {
        let machine_lease = self.machine_lease.as_mut().ok_or_else(|| {
            MobError::Internal(format!(
                "recovered host publication for '{}' lost exact attachment authority before predecessor abandonment",
                self.session_id
            ))
        })?;
        machine_lease
            .abandon_recovered_predecessor_inputs()
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "recovered host publication for '{}' could not abandon predecessor inputs: {error}",
                    self.session_id
                ))
            })
    }

    /// Publish the exact attachment sidecar and host residency under the same
    /// retained machine mutation gate, then release the runtime loop to serve.
    pub(super) async fn commit_serving_with_residency(
        self,
        residency_update: meerkat_runtime::meerkat_machine::MemberResidencyUpdate,
        incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        tracked_turn_journal: Option<
            Arc<dyn meerkat_runtime::member_observation::TrackedTurnJournal>,
        >,
    ) -> Result<
        (
            Arc<RuntimeSessionState>,
            meerkat_runtime::meerkat_machine::MemberResidencyPublication,
        ),
        MobError,
    > {
        let session_id = self.session_id.clone();
        let cleanup_spawner = self.cleanup_spawner.clone();
        let (result_tx, result_rx) = oneshot::channel();
        cleanup_spawner.spawn_detached(async move {
            let result = self
                .commit_serving_with_residency_owned(
                    residency_update,
                    incarnation,
                    tracked_turn_journal,
                )
                .await;
            // If the ambient host task was cancelled, a successful final
            // publication remains serving. Durable host authority was already
            // committed before this seam, and aborting here would resurrect
            // the exact delayed-publication ABA this transaction prevents.
            let _ = result_tx.send(result);
        });
        result_rx.await.map_err(|error| {
            MobError::Internal(format!(
                "owned final host publication for '{session_id}' ended without a result: {error}"
            ))
        })?
    }

    async fn commit_serving_with_residency_owned(
        mut self,
        residency_update: meerkat_runtime::meerkat_machine::MemberResidencyUpdate,
        incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        tracked_turn_journal: Option<
            Arc<dyn meerkat_runtime::member_observation::TrackedTurnJournal>,
        >,
    ) -> Result<
        (
            Arc<RuntimeSessionState>,
            meerkat_runtime::meerkat_machine::MemberResidencyPublication,
        ),
        MobError,
    > {
        let Some(machine_lease) = self.machine_lease.as_mut() else {
            let original = format!(
                "committed host publication for '{}' lost exact M authority",
                self.session_id
            );
            return Err(self.abort_after_publication_failure(original).await);
        };
        if !self.state.attachment_is_pending(machine_lease.witness()) {
            let original = format!(
                "host sidecar for '{}' was not pending at final publication",
                self.session_id
            );
            return Err(self.abort_after_publication_failure(original).await);
        }
        let state = Arc::clone(&self.state);
        let activation = Arc::clone(&state);
        let mut staged_activation = None;
        let Some(machine_lease) = self.machine_lease.as_mut() else {
            let original = format!(
                "host publication for '{}' lost exact M authority before commit",
                self.session_id
            );
            return Err(self.abort_after_publication_failure(original).await);
        };
        let commit = machine_lease
            .commit_with_member_residency(
                residency_update,
                incarnation,
                tracked_turn_journal,
                |witness| {
                    staged_activation = activation.stage_attachment_activation(witness);
                    staged_activation.as_ref().map(|_| ()).ok_or_else(|| {
                        meerkat_runtime::RuntimeDriverError::Internal(format!(
                            "sidecar for session '{}' could not activate at host residency publication",
                            witness.session_id()
                        ))
                    })
                },
            )
            .await;
        let (witness, residency_publication) = match commit {
            Ok(committed) => committed,
            Err(error) => {
                // Restore Active -> Pending synchronously while the exact
                // machine lease still retains M, before exact abort begins.
                drop(staged_activation);
                return Err(self
                    .abort_after_publication_failure(error.to_string())
                    .await);
            }
        };
        let Some(staged_activation) = staged_activation.take() else {
            let original = format!(
                "host publication for '{}' committed without exact sidecar activation",
                self.session_id
            );
            return Err(self.abort_after_publication_failure(original).await);
        };
        staged_activation.finalize();
        debug_assert!(state.attachment_is_active(&witness));
        self.machine_lease.take();
        self.boundary.take();
        self.armed = false;
        Ok((state, residency_publication))
    }

    pub(super) async fn abort(self) -> Result<(), MobError> {
        let mut this = self;
        let session_id = this.session_id.clone();
        let state = Arc::clone(&this.state);
        let machine_lease = this.machine_lease.take();
        let boundary = this.boundary.take();
        let cleanup_spawner = this.cleanup_spawner.clone();
        let (result_tx, result_rx) = oneshot::channel();
        cleanup_spawner.spawn_detached(async move {
            let result = Self::abort_owned_parts(session_id, state, machine_lease, boundary).await;
            let _ = result_tx.send(result);
        });
        this.armed = false;
        result_rx.await.map_err(|error| {
            MobError::Internal(format!(
                "owned committed host publication abort ended without a result: {error}"
            ))
        })?
    }
}

#[cfg(feature = "runtime-adapter")]
impl Drop for CommittedRuntimeSessionPublicationLease {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let session_id = self.session_id.clone();
        let state = Arc::clone(&self.state);
        let machine_lease = self.machine_lease.take();
        let boundary = self.boundary.take();
        self.cleanup_spawner.spawn_detached(async move {
            if let Err(error) =
                Self::abort_owned_parts(session_id.clone(), state, machine_lease, boundary).await
            {
                tracing::error!(
                    %session_id,
                    %error,
                    "cancelled committed host publication cleanup failed"
                );
            }
        });
    }
}

#[cfg(feature = "runtime-adapter")]
pub(super) async fn commit_pending_runtime_session_state_for_publication(
    _adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    abort_actor_witness: meerkat_session::LiveSessionActorWitness,
    runtime_sessions: &Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    candidate: Arc<RuntimeSessionState>,
    mut pending: PendingRuntimeExecutorAttachment,
    boundary: &mut Option<RuntimeTurnFinalizationBoundaryLease>,
) -> Result<CommittedRuntimeSessionPublicationLease, MobError> {
    boundary
        .as_ref()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "runtime publication for '{session_id}' lost B before attachment commit"
            ))
        })?
        .require_session(session_id)?;
    if abort_actor_witness.session_id() != session_id {
        pending
            .abort_under_runtime_turn_finalization_boundary()
            .await
            .map_err(|error| MobError::Internal(error.to_string()))?;
        return Err(MobError::Internal(format!(
            "live actor witness for '{}' cannot authorize host attachment publication for '{session_id}'",
            abort_actor_witness.session_id()
        )));
    }
    if !candidate.attachment_matches(pending.witness()) {
        pending
            .abort_under_runtime_turn_finalization_boundary()
            .await
            .map_err(|error| MobError::Internal(error.to_string()))?;
        return Err(MobError::Internal(format!(
            "runtime sidecar candidate did not match pending host attachment for '{session_id}'"
        )));
    }
    if runtime_sessions.read().await.contains_key(session_id) {
        pending
            .abort_under_runtime_turn_finalization_boundary()
            .await
            .map_err(|error| MobError::Internal(error.to_string()))?;
        return Err(MobError::Internal(format!(
            "runtime sidecar slot for '{session_id}' is already owned"
        )));
    }
    let cleanup_spawner = pending.cleanup_task_spawner();
    let machine_lease = match pending
        .try_commit_with_retained_publication_lease_under_runtime_turn_finalization_boundary()
        .await
    {
        Ok(lease) => lease,
        Err(error) => {
            let cleanup = pending
                .abort_under_runtime_turn_finalization_boundary()
                .await;
            return Err(MobError::Internal(match cleanup {
                Ok(()) => error.to_string(),
                Err(cleanup_error) => format!(
                    "{error}; exact pending host attachment cleanup failed: {cleanup_error}"
                ),
            }));
        }
    };
    let conflict = {
        let mut runtime_sessions = runtime_sessions.write().await;
        if runtime_sessions.contains_key(session_id) {
            true
        } else {
            runtime_sessions.insert(session_id.clone(), Arc::clone(&candidate));
            false
        }
    };
    if conflict {
        let cleanup = machine_lease
            .abort_under_runtime_turn_finalization_boundary()
            .await;
        return Err(MobError::Internal(match cleanup {
            Ok(_) => format!(
                "runtime sidecar slot for '{session_id}' changed before retained publication"
            ),
            Err(cleanup_error) => format!(
                "runtime sidecar slot for '{session_id}' changed before retained publication; exact attachment cleanup also failed: {cleanup_error}"
            ),
        }));
    }
    let boundary = boundary.take().ok_or_else(|| {
        MobError::Internal(format!(
            "runtime publication for '{session_id}' lost B at ownership transfer"
        ))
    })?;
    Ok(CommittedRuntimeSessionPublicationLease {
        session_id: session_id.clone(),
        state: candidate,
        cleanup_spawner,
        machine_lease: Some(machine_lease),
        boundary: Some(boundary),
        armed: true,
    })
}

#[cfg(feature = "runtime-adapter")]
// This is the exact B + sidecar-map + pending-attachment commit boundary;
// a params bag would duplicate the transaction authority already carried here.
#[allow(clippy::too_many_arguments)]
pub(super) async fn commit_pending_runtime_session_state(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    runtime_sessions: &Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    candidate: Arc<RuntimeSessionState>,
    replaced_state: Option<&Arc<RuntimeSessionState>>,
    mut pending: PendingRuntimeExecutorAttachment,
    boundary: &mut RuntimeTurnFinalizationBoundaryLease,
    on_committed: impl FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), MobError>,
) -> Result<Arc<RuntimeSessionState>, MobError> {
    boundary.require_session(session_id)?;
    if !candidate.attachment_matches(pending.witness()) {
        let abort_error = pending
            .abort_under_runtime_turn_finalization_boundary()
            .await
            .err();
        return Err(MobError::Internal(match abort_error {
            Some(error) => format!(
                "runtime sidecar candidate did not match the pending attachment for session '{session_id}', and attachment abort failed: {error}"
            ),
            None => format!(
                "runtime sidecar candidate did not match the pending attachment for session '{session_id}'"
            ),
        }));
    }

    if !candidate.queued_turn_owner.is_owned_by(candidate.witness()) {
        let abort_error = pending
            .abort_under_runtime_turn_finalization_boundary()
            .await
            .err();
        return Err(MobError::Internal(match abort_error {
            Some(error) => format!(
                "sidecar for '{session_id}' did not own its exact attachment-local queue, and attachment abort failed: {error}"
            ),
            None => {
                format!("sidecar for '{session_id}' did not own its exact attachment-local queue")
            }
        }));
    }
    if let Some(replaced) = replaced_state {
        if !replaced.attachment_is_active(replaced.witness()) || replaced.cleanup_scheduled() {
            let abort_error = pending
                .abort_under_runtime_turn_finalization_boundary()
                .await
                .err();
            return Err(MobError::Internal(match abort_error {
                Some(error) => format!(
                    "missing-live sidecar replacement for '{session_id}' lost exact A attachment authority, and B abort failed: {error}"
                ),
                None => format!(
                    "missing-live sidecar replacement for '{session_id}' lost exact A attachment authority"
                ),
            }));
        }
    }

    // Publish non-admitting B while retaining the map guard and reversible A
    // snapshot through the machine's exact serving commit. Queued work remains
    // owned by A and is never transferred into B's attachment-local queue.
    let mut runtime_sessions_guard = runtime_sessions.write().await;
    if let Err(error) = StagedRuntimeSessionMapPublication::validate_stage(
        &runtime_sessions_guard,
        session_id,
        replaced_state,
    ) {
        drop(runtime_sessions_guard);
        let abort_error = pending
            .abort_under_runtime_turn_finalization_boundary()
            .await
            .err();
        return Err(match abort_error {
            Some(abort_error) => MobError::Internal(format!(
                "{error}; exact attachment abort also failed: {abort_error}"
            )),
            None => error,
        });
    }
    // Validation and staging share the same write guard, so the slot cannot
    // change between the fallible check and reversible publication.
    let staged_map_publication = StagedRuntimeSessionMapPublication::stage_validated(
        &mut runtime_sessions_guard,
        session_id,
        Arc::clone(&candidate),
    );

    let activation = Arc::clone(&candidate);
    let mut staged_activation = None;
    let commit_result = pending
        .try_commit_with_under_runtime_turn_finalization_boundary(
            replaced_state.is_some(),
            |witness| {
                staged_activation = activation.stage_attachment_activation(witness);
                if staged_activation.is_none() {
                    return Err(meerkat_runtime::RuntimeDriverError::Internal(format!(
                        "runtime sidecar for session '{}' could not stage activation at exact attachment commit",
                        witness.session_id()
                    )));
                }
                on_committed(witness).map_err(|error| {
                    meerkat_runtime::RuntimeDriverError::Internal(error.to_string())
                })?;
                Ok(())
            },
        )
        .await;
    let committed = match commit_result {
        Ok(committed) => committed,
        Err(error) => {
            // Restore Active -> Pending before consuming the exact M lease in
            // abort. Any callback-owned RAII publication remains armed until
            // its caller observes this failure.
            drop(staged_activation);
            drop(staged_map_publication);
            drop(runtime_sessions_guard);
            if let Err(cleanup_error) = pending
                .abort_under_runtime_turn_finalization_boundary()
                .await
            {
                return Err(MobError::Internal(format!(
                    "{error}; exact attachment cleanup failed: {cleanup_error}"
                )));
            }
            return Err(MobError::Internal(error.to_string()));
        }
    };
    let Some(staged_activation) = staged_activation.take() else {
        return Err(MobError::Internal(format!(
            "attachment commit for session '{session_id}' omitted exact sidecar activation"
        )));
    };
    if !candidate.attachment_is_active(&committed) {
        drop(staged_activation);
        drop(staged_map_publication);
        drop(runtime_sessions_guard);
        let cleanup_error = match adapter
            .prepare_executor_attachment_retirement_under_runtime_turn_boundary(&committed)
            .await
        {
            Ok(Some(retirement)) => retirement
                .commit_under_runtime_turn_finalization_boundary()
                .await
                .err(),
            Ok(None) => Some(meerkat_runtime::RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "committed attachment for session '{session_id}' changed before activation rollback"
                ),
            }),
            Err(error) => Some(error),
        };
        return Err(MobError::Internal(match cleanup_error {
            Some(error) => format!(
                "runtime sidecar for session '{session_id}' did not activate at attachment commit, and exact cleanup failed: {error}"
            ),
            None => format!(
                "runtime sidecar for session '{session_id}' did not activate at attachment commit"
            ),
        }));
    }
    staged_activation.finalize();
    staged_map_publication.finalize();
    drop(runtime_sessions_guard);
    Ok(candidate)
}

#[cfg(feature = "runtime-adapter")]
pub(super) async fn prepare_prepared_runtime_session_state(
    prepared: &mut PreparedSessionMaterialization,
    actor_witness: meerkat_session::LiveSessionActorWitness,
    session_service: Arc<dyn MobSessionService>,
    adapter: Arc<MeerkatMachine>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    boundary: &mut RuntimeTurnFinalizationBoundaryLease,
) -> Result<(Arc<RuntimeSessionState>, PendingRuntimeExecutorAttachment), MobError> {
    let session_id = prepared.session_id().clone();
    boundary.require_session(&session_id)?;
    let candidate_slot: Arc<StdMutex<Option<Arc<RuntimeSessionState>>>> =
        Arc::new(StdMutex::new(None));
    let factory_slot = Arc::clone(&candidate_slot);
    let executor_session_id = session_id.clone();
    let executor_adapter = Arc::clone(&adapter);
    let executor_runtime_sessions = Arc::clone(&runtime_sessions);
    let outcome = prepared
        .ensure_executor_attachment_under_runtime_turn_finalization_boundary(move |witness| {
            let state = Arc::new(RuntimeSessionState::for_attachment(witness, actor_witness));
            *factory_slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&state));
            Box::new(MobSessionRuntimeExecutor::new(
                session_service,
                executor_adapter,
                workgraph_service,
                executor_session_id,
                state,
                executor_runtime_sessions,
            ))
        })
        .await
        .map_err(|error| MobError::Internal(error.to_string()))?;
    let pending = match outcome {
        EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
        EnsureRuntimeExecutorAttachment::Existing(witness) => {
            return Err(MobError::Internal(format!(
                "prepared session materialization unexpectedly found committed attachment {witness:?} for session '{session_id}'"
            )));
        }
    };
    let candidate = candidate_slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let Some(candidate) = candidate else {
        let abort_error = pending
            .abort_under_runtime_turn_finalization_boundary()
            .await
            .err();
        return Err(MobError::Internal(match abort_error {
            Some(error) => format!(
                "prepared executor factory did not publish an exact sidecar candidate for session '{session_id}', and attachment abort failed: {error}"
            ),
            None => format!(
                "prepared executor factory did not publish an exact sidecar candidate for session '{session_id}'"
            ),
        }));
    };
    Ok((candidate, pending))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        DeferredTurnEventOutcome, MultiBackendProvisioner,
        defer_turn_events_until_machine_completion, runtime_completion_to_mob_result,
        session_turn_error_to_mob_error,
    };
    #[cfg(feature = "runtime-adapter")]
    use super::{MemberSessionDisposalArc, RuntimeSessionDisposalTarget, RuntimeSessionState};
    use crate::error::MobError;
    use meerkat_core::service::SessionError;
    use meerkat_core::types::SessionId;
    use serde_json::json;

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn disposal_retry_readmits_exact_retirement_uncertain_draining_sidecar() {
        use meerkat_core::lifecycle::core_executor::{
            CoreApplyOutput, CoreExecutor, CoreExecutorError, CoreExecutorPostStopCleanupHandle,
        };
        use meerkat_core::lifecycle::run_primitive::RunPrimitive;
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::sync::{Notify, RwLock};

        struct BlockingCleanupHandle {
            entered: Arc<Notify>,
            release: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutorPostStopCleanupHandle for BlockingCleanupHandle {
            async fn cleanup_after_runtime_stop_terminalized(
                &self,
            ) -> Result<(), CoreExecutorError> {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(())
            }
        }

        struct BlockingCleanupExecutor {
            entered: Arc<Notify>,
            release: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingCleanupExecutor {
            fn machine_managed_post_stop_unregister(&self) -> bool {
                true
            }

            fn post_stop_cleanup_handle(
                &self,
            ) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
                Some(Arc::new(BlockingCleanupHandle {
                    entered: Arc::clone(&self.entered),
                    release: Arc::clone(&self.release),
                }))
            }

            async fn apply(
                &mut self,
                _run_id: meerkat_core::RunId,
                _primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                Err(CoreExecutorError::Internal(
                    "disposal retry fixture must not apply turns".to_string(),
                ))
            }

            async fn cancel_after_boundary(
                &mut self,
                _reason: String,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }

            async fn stop_runtime_executor(
                &mut self,
                _reason: String,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let cleanup_entered = Arc::new(Notify::new());
        let cleanup_release = Arc::new(Notify::new());
        machine
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingCleanupExecutor {
                    entered: Arc::clone(&cleanup_entered),
                    release: Arc::clone(&cleanup_release),
                }),
            )
            .await
            .expect("register disposal retry fixture");
        let witness = machine
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("registered fixture must expose its serving attachment");
        let state = Arc::new(RuntimeSessionState::for_attachment_without_actor_witness(
            witness.clone(),
        ));
        state
            .stage_attachment_activation(&witness)
            .expect("activate exact sidecar")
            .finalize();

        let runtime_sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            Arc::clone(&state),
        )])));
        assert!(state.mark_retirement_uncertain());
        assert!(!state.cleanup_scheduled());

        let unregister = {
            let machine = Arc::clone(&machine);
            let witness = witness.clone();
            tokio::spawn(async move {
                machine
                    .unregister_executor_attachment_if_current(&witness)
                    .await
            })
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            cleanup_entered.notified(),
        )
        .await
        .expect("canonical unregister must enter retained post-stop cleanup");
        assert!(
            machine
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "Draining must hide the serving attachment projection"
        );

        let captured =
            MemberSessionDisposalArc::capture_exact_runtime_state_for_disposal_with_adapter(
                &machine,
                &runtime_sessions,
                &session_id,
            )
            .await
            .expect("retry must accept the machine-proven retained cleanup sidecar");
        match captured {
            RuntimeSessionDisposalTarget::ExactAttachment {
                witness: captured_witness,
                sidecar: Some(captured_state),
                ..
            } => {
                assert_eq!(captured_witness, witness);
                assert!(Arc::ptr_eq(&captured_state, &state));
            }
            _ => panic!("retry did not recover the exact retained cleanup attachment"),
        }

        let foreign_machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let foreign_session_id = SessionId::new();
        let foreign_cleanup_entered = Arc::new(Notify::new());
        let foreign_cleanup_release = Arc::new(Notify::new());
        foreign_cleanup_release.notify_one();
        foreign_machine
            .register_session_with_executor(
                foreign_session_id.clone(),
                Box::new(BlockingCleanupExecutor {
                    entered: foreign_cleanup_entered,
                    release: foreign_cleanup_release,
                }),
            )
            .await
            .expect("register stale-sidecar fixture");
        let foreign_witness = foreign_machine
            .current_executor_attachment_witness(&foreign_session_id)
            .await
            .expect("stale-sidecar fixture must expose its serving attachment");
        runtime_sessions.write().await.insert(
            session_id.clone(),
            Arc::new(RuntimeSessionState::for_attachment_without_actor_witness(
                foreign_witness,
            )),
        );
        let stale_error =
            match MemberSessionDisposalArc::capture_exact_runtime_state_for_disposal_with_adapter(
                &machine,
                &runtime_sessions,
                &session_id,
            )
            .await
            {
                Ok(_) => panic!("machine validation accepted a stale sidecar witness"),
                Err(error) => error,
            };
        assert!(
            stale_error
                .to_string()
                .contains("cannot capture an exact machine/sidecar pair")
        );
        runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::clone(&state));
        foreign_machine
            .unregister_session(&foreign_session_id)
            .await
            .expect("clean stale-sidecar fixture");

        cleanup_release.notify_one();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), unregister)
                .await
                .expect("released unregister must complete")
                .expect("unregister task must not panic")
                .expect("unregister must retain exact authority")
        );
    }

    #[test]
    fn runtime_callback_pending_maps_to_typed_mob_error() {
        let session_id = SessionId::new();
        let err = runtime_completion_to_mob_result(
            &session_id,
            Ok(
                meerkat_runtime::completion::CompletionOutcome::CallbackPending {
                    tool_name: "external_mock".to_string(),
                    args: json!({ "value": "browser" }),
                },
            ),
        )
        .expect_err("callback pending should remain resumable mob state");

        match err {
            MobError::CallbackPending {
                session_id: actual_session_id,
                tool_name,
                args,
            } => {
                assert_eq!(actual_session_id, session_id);
                assert_eq!(tool_name, "external_mock");
                assert_eq!(args, json!({ "value": "browser" }));
            }
            other => panic!("expected callback-pending mob error, got {other:?}"),
        }
    }

    #[test]
    fn runtime_completion_wait_error_maps_to_typed_mob_error_without_result_class() {
        let session_id = SessionId::new();
        let err = runtime_completion_to_mob_result(
            &session_id,
            Err(
                meerkat_runtime::completion::CompletionWaitError::AuthorityUnavailable(
                    "generated runtime completion authority missing".to_string(),
                ),
            ),
        )
        .expect_err("waiter failure should not become a successful mob result");

        match err {
            MobError::Internal(message) => {
                assert!(message.contains("runtime completion waiter failed"));
                assert!(message.contains("generated runtime completion authority missing"));
            }
            other => panic!("expected internal mob waiter error, got {other:?}"),
        }
    }

    #[test]
    fn direct_session_callback_pending_maps_to_typed_mob_error() {
        let session_id = SessionId::new();
        let err = session_turn_error_to_mob_error(
            &session_id,
            SessionError::Agent(meerkat_core::error::AgentError::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: json!({ "value": "browser" }),
            }),
        );

        match err {
            MobError::CallbackPending {
                session_id: actual_session_id,
                tool_name,
                args,
            } => {
                assert_eq!(actual_session_id, session_id);
                assert_eq!(tool_name, "external_mock");
                assert_eq!(args, json!({ "value": "browser" }));
            }
            other => panic!("expected callback-pending mob error, got {other:?}"),
        }
    }

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn deferred_turn_delivery_closes_channel_without_synthetic_events() {
        let session_id = SessionId::new();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(1);
        let (queued_event_tx, deferred_delivery) =
            defer_turn_events_until_machine_completion(&session_id, Some(event_tx));

        let deferred_delivery = deferred_delivery.expect("deferred delivery handle");
        deferred_delivery.release(DeferredTurnEventOutcome::Succeeded);
        drop(queued_event_tx);

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("channel should close promptly after release");
        assert!(
            result.is_none(),
            "deferred buffer must not fabricate events; channel should close empty"
        );
    }

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn deferred_turn_delivery_streams_deltas_and_commit_gates_terminal_events() {
        let session_id = SessionId::new();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
        let (queued_event_tx, deferred_delivery) =
            defer_turn_events_until_machine_completion(&session_id, Some(event_tx));
        let queued_event_tx = queued_event_tx.expect("queued event sender");
        let deferred_delivery = deferred_delivery.expect("deferred delivery handle");
        queued_event_tx
            .send(meerkat_core::EventEnvelope::new_session(
                session_id.clone(),
                1,
                None,
                meerkat_core::AgentEvent::TextDelta {
                    delta: "live".to_string(),
                },
            ))
            .await
            .expect("send live delta");
        queued_event_tx
            .send(meerkat_core::EventEnvelope::new_session(
                session_id.clone(),
                2,
                None,
                meerkat_core::AgentEvent::RunCompleted {
                    session_id,
                    result: "done".to_string(),
                    structured_output: None,
                    extraction_required: false,
                    usage: Default::default(),
                    terminal_cause_kind: None,
                },
            ))
            .await
            .expect("send terminal event");
        let live = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("delta should stream before release")
            .expect("live event");
        assert!(matches!(
            live.payload,
            meerkat_core::AgentEvent::TextDelta { .. }
        ));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), event_rx.recv())
                .await
                .is_err(),
            "terminal event must remain buffered before commit release"
        );
        deferred_delivery.release(DeferredTurnEventOutcome::Succeeded);
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("terminal should flush after release")
            .expect("terminal event");
        assert!(matches!(
            terminal.payload,
            meerkat_core::AgentEvent::RunCompleted { .. }
        ));
    }

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn deferred_turn_delivery_streams_extraction_boundary_before_success_commit() {
        let session_id = SessionId::new();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
        let (queued_event_tx, deferred_delivery) =
            defer_turn_events_until_machine_completion(&session_id, Some(event_tx));
        let queued_event_tx = queued_event_tx.expect("queued event sender");
        let deferred_delivery = deferred_delivery.expect("deferred delivery handle");

        queued_event_tx
            .send(meerkat_core::EventEnvelope::new_session(
                session_id.clone(),
                1,
                None,
                meerkat_core::AgentEvent::RunCompleted {
                    session_id: session_id.clone(),
                    result: "committed primary output".to_string(),
                    structured_output: None,
                    extraction_required: true,
                    usage: Default::default(),
                    terminal_cause_kind: None,
                },
            ))
            .await
            .expect("send extraction boundary");
        let boundary = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("extraction boundary should stream before release")
            .expect("extraction boundary event");
        assert!(matches!(
            boundary.payload,
            meerkat_core::AgentEvent::RunCompleted {
                extraction_required: true,
                ..
            }
        ));

        queued_event_tx
            .send(meerkat_core::EventEnvelope::new_session(
                session_id.clone(),
                2,
                None,
                meerkat_core::AgentEvent::ExtractionSucceeded {
                    session_id,
                    structured_output: json!({"answer": 42}),
                    schema_warnings: None,
                },
            ))
            .await
            .expect("send extraction success terminal");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), event_rx.recv())
                .await
                .is_err(),
            "extraction success must remain buffered before commit release"
        );

        deferred_delivery.release(DeferredTurnEventOutcome::Succeeded);
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("extraction success should flush after release")
            .expect("extraction success event");
        assert!(matches!(
            terminal.payload,
            meerkat_core::AgentEvent::ExtractionSucceeded { .. }
        ));
    }

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn deferred_turn_delivery_streams_extraction_boundary_before_failure_commit() {
        let session_id = SessionId::new();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
        let (queued_event_tx, deferred_delivery) =
            defer_turn_events_until_machine_completion(&session_id, Some(event_tx));
        let queued_event_tx = queued_event_tx.expect("queued event sender");
        let deferred_delivery = deferred_delivery.expect("deferred delivery handle");

        queued_event_tx
            .send(meerkat_core::EventEnvelope::new_session(
                session_id.clone(),
                1,
                None,
                meerkat_core::AgentEvent::RunCompleted {
                    session_id: session_id.clone(),
                    result: "committed primary output".to_string(),
                    structured_output: None,
                    extraction_required: true,
                    usage: Default::default(),
                    terminal_cause_kind: None,
                },
            ))
            .await
            .expect("send extraction boundary");
        let boundary = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("extraction boundary should stream before release")
            .expect("extraction boundary event");
        assert!(matches!(
            boundary.payload,
            meerkat_core::AgentEvent::RunCompleted {
                extraction_required: true,
                ..
            }
        ));

        queued_event_tx
            .send(meerkat_core::EventEnvelope::new_session(
                session_id.clone(),
                2,
                None,
                meerkat_core::AgentEvent::ExtractionFailed {
                    session_id,
                    last_output: "committed primary output".to_string(),
                    attempts: 1,
                    reason: "invalid shape".to_string(),
                },
            ))
            .await
            .expect("send extraction failure terminal");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), event_rx.recv())
                .await
                .is_err(),
            "extraction failure must remain buffered before commit release"
        );

        deferred_delivery.release(DeferredTurnEventOutcome::ExtractionFailed);
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("extraction failure should flush after release")
            .expect("extraction failure event");
        assert!(matches!(
            terminal.payload,
            meerkat_core::AgentEvent::ExtractionFailed { .. }
        ));
    }

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn deferred_turn_delivery_commit_gates_all_interaction_terminals() {
        async fn assert_gated(event: meerkat_core::AgentEvent, outcome: DeferredTurnEventOutcome) {
            let session_id = SessionId::new();
            let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(2);
            let (queued_event_tx, deferred_delivery) =
                defer_turn_events_until_machine_completion(&session_id, Some(event_tx));
            let queued_event_tx = queued_event_tx.expect("queued event sender");
            let deferred_delivery = deferred_delivery.expect("deferred delivery handle");
            queued_event_tx
                .send(meerkat_core::EventEnvelope::new_session(
                    session_id, 1, None, event,
                ))
                .await
                .expect("send interaction terminal");
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(25), event_rx.recv())
                    .await
                    .is_err(),
                "interaction terminal must not escape before committed completion"
            );
            deferred_delivery.release(outcome);
            let released = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
                .await
                .expect("matching interaction terminal should release")
                .expect("released interaction terminal");
            assert!(
                matches!(
                    (outcome, released.payload),
                    (
                        DeferredTurnEventOutcome::Succeeded,
                        meerkat_core::AgentEvent::InteractionComplete { .. }
                    ) | (
                        DeferredTurnEventOutcome::CallbackPending,
                        meerkat_core::AgentEvent::InteractionCallbackPending { .. }
                    ) | (
                        DeferredTurnEventOutcome::Failed,
                        meerkat_core::AgentEvent::InteractionFailed { .. }
                    ) | (
                        DeferredTurnEventOutcome::ExtractionFailed,
                        meerkat_core::AgentEvent::InteractionFailed {
                            reason:
                                meerkat_core::event::InteractionFailureReason::ExtractionFailed { .. },
                            ..
                        }
                    )
                ),
                "released interaction terminal must match committed outcome"
            );
        }

        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());
        assert_gated(
            meerkat_core::AgentEvent::InteractionComplete {
                interaction_id,
                result: "done".to_string(),
                structured_output: None,
            },
            DeferredTurnEventOutcome::Succeeded,
        )
        .await;
        assert_gated(
            meerkat_core::AgentEvent::InteractionCallbackPending {
                interaction_id,
                tool_name: "external".to_string(),
                args: json!({"value": 1}),
            },
            DeferredTurnEventOutcome::CallbackPending,
        )
        .await;
        assert_gated(
            meerkat_core::AgentEvent::InteractionFailed {
                interaction_id,
                reason: meerkat_core::event::InteractionFailureReason::abandoned("runtime failed"),
            },
            DeferredTurnEventOutcome::Failed,
        )
        .await;
        assert_gated(
            meerkat_core::AgentEvent::InteractionFailed {
                interaction_id,
                reason: meerkat_core::event::InteractionFailureReason::ExtractionFailed {
                    last_output: "primary output".to_string(),
                    attempts: 1,
                    reason: "invalid shape".to_string(),
                },
            },
            DeferredTurnEventOutcome::ExtractionFailed,
        )
        .await;
    }

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn batched_turn_context_resolves_every_llm_identity_observer() {
        let (identity_a_tx, identity_a_rx) = tokio::sync::oneshot::channel();
        let (identity_b_tx, identity_b_rx) = tokio::sync::oneshot::channel();
        let mut context = super::QueuedTurnContext::new(None, Some(identity_a_tx));
        context.append(super::QueuedTurnContext::new(None, Some(identity_b_tx)));
        let (_, mut identity_application) = context.into_executor_parts();
        let expected = meerkat_core::SessionLlmIdentity {
            model: "batched-model".to_string(),
            provider: meerkat_core::Provider::Other,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };

        identity_application.resolve_success(Some(expected.clone()));

        assert_eq!(
            identity_a_rx
                .await
                .expect("first identity observer remains live")
                .expect("first identity application succeeds"),
            Some(expected.clone())
        );
        assert_eq!(
            identity_b_rx
                .await
                .expect("second identity observer remains live")
                .expect("second identity application succeeds"),
            Some(expected)
        );
    }

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn batched_turn_event_fanout_backpressures_without_losing_ordered_events() {
        let session_id = SessionId::new();
        let (event_a_tx, mut event_a_rx) = tokio::sync::mpsc::channel(1);
        let (event_b_tx, mut event_b_rx) = tokio::sync::mpsc::channel(1);
        let fanout_tx =
            super::fan_out_turn_event_txs_with_capacity(vec![event_a_tx, event_b_tx], 1)
                .expect("two destinations require a fanout sender");

        let producer_session_id = session_id.clone();
        let mut producer = tokio::spawn(async move {
            for seq in 1..=4 {
                fanout_tx
                    .send(meerkat_core::EventEnvelope::new_session(
                        producer_session_id.clone(),
                        seq,
                        None,
                        meerkat_core::AgentEvent::TextDelta {
                            delta: format!("delta-{seq}"),
                        },
                    ))
                    .await
                    .expect("fanout router should remain live");
            }
        });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut producer)
                .await
                .is_err(),
            "bounded fanout must backpressure instead of dropping a full destination"
        );

        for expected_seq in 1..=4 {
            let event_a =
                tokio::time::timeout(std::time::Duration::from_secs(1), event_a_rx.recv())
                    .await
                    .expect("first destination should make progress")
                    .expect("first destination remains live");
            let event_b =
                tokio::time::timeout(std::time::Duration::from_secs(1), event_b_rx.recv())
                    .await
                    .expect("second destination should make progress")
                    .expect("second destination remains live");
            assert_eq!(event_a.seq, expected_seq);
            assert_eq!(event_b.seq, expected_seq);
            assert_eq!(event_a.event_id, event_b.event_id);
            assert_eq!(event_a.source, event_b.source);
            assert_eq!(event_a.source.session_id(), Some(&session_id));
            match (&event_a.payload, &event_b.payload) {
                (
                    meerkat_core::AgentEvent::TextDelta { delta: delta_a },
                    meerkat_core::AgentEvent::TextDelta { delta: delta_b },
                ) => assert_eq!(delta_a, delta_b),
                events => panic!("both destinations should receive matching deltas: {events:?}"),
            }
        }

        tokio::time::timeout(std::time::Duration::from_secs(1), producer)
            .await
            .expect("producer should resume after destination capacity is released")
            .expect("producer task should succeed");
    }

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn replacement_attachment_gets_a_fresh_queue_and_old_cleanup_cannot_reach_it() {
        use meerkat_core::lifecycle::core_executor::{
            CoreApplyOutput, CoreExecutor, CoreExecutorError,
        };
        use meerkat_core::lifecycle::run_primitive::RunPrimitive;
        use meerkat_runtime::EnsureRuntimeExecutorAttachment;
        use std::sync::Arc;

        struct IdleExecutor;

        #[async_trait::async_trait]
        impl CoreExecutor for IdleExecutor {
            async fn apply(
                &mut self,
                _run_id: meerkat_core::RunId,
                _primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                Err(CoreExecutorError::Internal(
                    "queue-isolation executor must not run".to_string(),
                ))
            }

            async fn cancel_after_boundary(
                &mut self,
                _reason: String,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }

            async fn stop_runtime_executor(
                &mut self,
                _reason: String,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register queue-isolation session");

        let pending_a = match machine
            .ensure_session_with_executor_factory(session_id.clone(), |_| Box::new(IdleExecutor))
            .await
            .expect("prepare attachment A")
        {
            EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
            EnsureRuntimeExecutorAttachment::Existing(witness) => {
                panic!("fresh queue-isolation session unexpectedly found {witness:?}")
            }
        };
        let witness_a = pending_a.witness().clone();
        let owner_a = Arc::new(super::RuntimeQueuedTurnOwner::new(witness_a.clone()));
        let input_id = meerkat_core::InputId::new();
        let (a_tx, _a_rx) = tokio::sync::mpsc::channel(1);
        let _a_registration = owner_a
            .register_turn_context(&witness_a, input_id.clone(), Some(a_tx), None)
            .expect("register A-local request context");
        pending_a.abort().await.expect("abort attachment A");

        let pending_b = match machine
            .ensure_session_with_executor_factory(session_id, |_| Box::new(IdleExecutor))
            .await
            .expect("prepare attachment B")
        {
            EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
            EnsureRuntimeExecutorAttachment::Existing(witness) => {
                panic!("replacement unexpectedly reused attachment {witness:?}")
            }
        };
        let witness_b = pending_b.witness().clone();
        let owner_b = Arc::new(super::RuntimeQueuedTurnOwner::new(witness_b.clone()));
        assert_ne!(witness_a, witness_b);
        assert!(
            owner_b
                .take_turn_context_for_inputs(&witness_b, std::slice::from_ref(&input_id))
                .is_none(),
            "replacement B must begin with a fresh attachment-local queue"
        );

        let (b_tx, _b_rx) = tokio::sync::mpsc::channel(1);
        let _b_registration = owner_b
            .register_turn_context(&witness_b, input_id.clone(), Some(b_tx), None)
            .expect("register B-local request context under the same logical input id");
        assert!(owner_a.clear_if_owned_by(&witness_a));
        assert!(
            owner_b
                .take_turn_context_for_inputs(&witness_b, std::slice::from_ref(&input_id))
                .is_some(),
            "cleanup for A must not remove or expose B's request context"
        );

        pending_b.abort().await.expect("abort attachment B");
    }

    #[cfg(feature = "runtime-adapter")]
    #[tokio::test]
    async fn retirement_uncertain_is_non_admitting_without_clearing_the_exact_queue() {
        use meerkat_core::lifecycle::core_executor::{
            CoreApplyOutput, CoreExecutor, CoreExecutorError,
        };
        use meerkat_core::lifecycle::run_primitive::RunPrimitive;
        use meerkat_runtime::EnsureRuntimeExecutorAttachment;
        use std::sync::Arc;

        struct IdleExecutor;

        #[async_trait::async_trait]
        impl CoreExecutor for IdleExecutor {
            async fn apply(
                &mut self,
                _run_id: meerkat_core::RunId,
                _primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                Err(CoreExecutorError::Internal(
                    "retirement-uncertain executor must not run".to_string(),
                ))
            }

            async fn cancel_after_boundary(
                &mut self,
                _reason: String,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }

            async fn stop_runtime_executor(
                &mut self,
                _reason: String,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register retirement-uncertain fixture");
        let pending = match machine
            .ensure_session_with_executor_factory(session_id, |_| Box::new(IdleExecutor))
            .await
            .expect("prepare retirement-uncertain attachment")
        {
            EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
            EnsureRuntimeExecutorAttachment::Existing(witness) => {
                panic!("fresh retirement-uncertain fixture unexpectedly found {witness:?}")
            }
        };
        let state = Arc::new(
            super::RuntimeSessionState::for_attachment_without_actor_witness(
                pending.witness().clone(),
            ),
        );
        state
            .stage_attachment_activation(pending.witness())
            .expect("activate exact sidecar")
            .finalize();
        let input_id = meerkat_core::InputId::new();
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(1);
        let _registration = state
            .register_turn_context(input_id.clone(), Some(event_tx), None)
            .expect("register exact attachment-local context")
            .expect("event sender should create a registration");

        assert!(state.mark_retirement_uncertain());
        assert!(state.retirement_is_uncertain());
        assert!(!state.attachment_is_active(state.witness()));
        assert!(
            !state.cleanup_scheduled(),
            "uncertain retirement must not pretend post-stop cleanup was scheduled"
        );
        assert!(
            state
                .take_turn_context_for_inputs(std::slice::from_ref(&input_id))
                .await
                .is_some(),
            "transferring M must preserve queued work until canonical post-stop cleanup"
        );
        assert!(state.mark_cleanup_scheduled());
        assert!(state.cleanup_scheduled());

        pending
            .abort()
            .await
            .expect("abort retirement-uncertain fixture");
    }

    #[cfg(feature = "runtime-adapter")]
    #[test]
    fn validated_external_peer_spec_preserves_the_validated_peer_name() {
        // Post-#24 `PeerId` is a typed UUID; `PeerId::parse` rejects the
        // legacy `ed25519:<alias>` shape that this test previously pinned.
        // The contract being asserted here is preservation of the peer
        // name + address through validation, so we feed a round-trippable
        // UUID string and round-trip through `PeerId::to_string()` on the
        // way out.
        let pubkey = [3u8; 32];
        let peer_id = meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey);
        let peer_id_str = peer_id.to_string();
        let spec = MultiBackendProvisioner::validated_external_peer_spec(
            "mob/worker/member-1",
            &peer_id_str,
            "tcp://example.invalid/member-1",
            pubkey,
        )
        .expect("external peer spec should validate");

        assert_eq!(spec.name.as_str(), "mob/worker/member-1");
        assert_eq!(spec.peer_id.to_string(), peer_id_str);
        assert_eq!(spec.address.to_string(), "tcp://example.invalid/member-1");
    }

    #[cfg(feature = "runtime-adapter")]
    #[test]
    fn session_owned_eager_member_create_gets_runtime_execution_kind_stamp() {
        let mut req = meerkat_core::service::CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: "hello".to_string().into(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                initial_turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
                        ..Default::default()
                    },
                ),
                ..Default::default()
            }),
            labels: None,
        };

        super::stamp_eager_session_owned_initial_turn_metadata(&mut req);

        let metadata = req
            .build
            .as_ref()
            .and_then(|build| build.initial_turn_metadata.as_ref())
            .expect("eager runtime-owned member create should be stamped");
        assert_eq!(
            metadata.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
        assert_eq!(
            metadata.handling_mode,
            Some(meerkat_core::types::HandlingMode::Queue)
        );
    }
}

#[cfg(feature = "runtime-adapter")]
pub(super) struct MobSessionRuntimeExecutor {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Arc<MeerkatMachine>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    bridge_session_id: SessionId,
    state: Arc<RuntimeSessionState>,
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
}

#[cfg(feature = "runtime-adapter")]
impl MobSessionRuntimeExecutor {
    pub(super) fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Arc<MeerkatMachine>,
        workgraph_service: Option<meerkat::WorkGraphService>,
        bridge_session_id: SessionId,
        state: Arc<RuntimeSessionState>,
        runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            workgraph_service,
            bridge_session_id,
            state,
            runtime_sessions,
        }
    }
}

#[cfg(feature = "runtime-adapter")]
struct MobSessionRuntimeBoundaryHandle {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Arc<MeerkatMachine>,
    bridge_session_id: SessionId,
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutorBoundaryHandle for MobSessionRuntimeBoundaryHandle {
    async fn cancel_after_boundary(
        &self,
        expected_run_id: &CoreRunId,
        _reason: String,
    ) -> Result<(), CoreExecutorError> {
        self.session_service
            .cancel_after_boundary_with_machine_authority(
                &self.bridge_session_id,
                expected_run_id,
                self.runtime_adapter.session_control_authority(),
            )
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }

    async fn prepare_system_context_at_boundary(
        &self,
        expected_run_id: &CoreRunId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<
        meerkat_core::lifecycle::CoreBoundaryStageOutput,
        meerkat_core::CoreBoundaryStageError,
    > {
        self.session_service
            .prepare_runtime_system_context_for_active_turn(
                &self.bridge_session_id,
                expected_run_id,
                appends,
            )
            .await
    }
}

#[cfg(feature = "runtime-adapter")]
struct MobSessionServiceInterruptHandle {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Arc<MeerkatMachine>,
    bridge_session_id: SessionId,
}

#[cfg(feature = "runtime-adapter")]
struct MobSessionTerminalPublicationHandle {
    session_service: Arc<dyn MobSessionService>,
    bridge_session_id: SessionId,
}

#[cfg(feature = "runtime-adapter")]
struct MobSessionTurnFinalizationBoundaryHandle {
    session_service: Arc<dyn MobSessionService>,
    bridge_session_id: SessionId,
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutorTurnFinalizationBoundaryHandle for MobSessionTurnFinalizationBoundaryHandle {
    async fn acquire(
        &self,
    ) -> Result<Box<dyn CoreExecutorTurnFinalizationGuard>, CoreExecutorError> {
        self.session_service
            .acquire_runtime_turn_finalization_guard(&self.bridge_session_id)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
}

#[cfg(feature = "runtime-adapter")]
pub(super) struct MobSessionRuntimePostStopCleanupHandle {
    session_service: Arc<dyn MobSessionService>,
    bridge_session_id: SessionId,
    state: Arc<RuntimeSessionState>,
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
}

#[cfg(feature = "runtime-adapter")]
struct MobPreparedSessionActorCleanupHandle {
    session_service: Arc<dyn MobSessionService>,
    actor_witness_slot: meerkat_session::LiveSessionActorWitnessSlot,
}

#[cfg(feature = "runtime-adapter")]
impl MobPreparedSessionActorCleanupHandle {
    async fn cleanup(&self, under_turn_boundary: bool) -> Result<(), CoreExecutorError> {
        let Some(actor_witness) = self.actor_witness_slot.witness() else {
            return Ok(());
        };
        let _turn_finalization_guard = if under_turn_boundary {
            None
        } else {
            Some(
                self.session_service
                    .acquire_runtime_turn_finalization_guard(actor_witness.session_id())
                    .await
                    .map_err(CoreExecutorError::apply_failed_from_session_error)?,
            )
        };
        self.session_service
            .discard_live_session_actor_under_runtime_turn_boundary(&actor_witness)
            .await
            .map(|_| ())
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutorPostStopCleanupHandle for MobPreparedSessionActorCleanupHandle {
    async fn cleanup_after_runtime_stop_terminalized(&self) -> Result<(), CoreExecutorError> {
        self.cleanup(false).await
    }

    async fn cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
    ) -> Result<(), CoreExecutorError> {
        self.cleanup(true).await
    }
}

#[cfg(feature = "runtime-adapter")]
pub(super) async fn install_prepared_mob_session_executor_handles(
    session_service: Arc<dyn MobSessionService>,
    adapter: Arc<MeerkatMachine>,
    prepared: &PreparedSessionMaterialization,
    actor_witness_slot: meerkat_session::LiveSessionActorWitnessSlot,
) -> Result<(), meerkat_runtime::RuntimeDriverError> {
    let session_id = prepared.session_id().clone();
    adapter
        .install_prepared_session_executor_handles(
            prepared.bindings(),
            Arc::new(MobSessionServiceInterruptHandle {
                session_service: Arc::clone(&session_service),
                runtime_adapter: Arc::clone(&adapter),
                bridge_session_id: session_id.clone(),
            }),
            Arc::new(MobPreparedSessionActorCleanupHandle {
                session_service,
                actor_witness_slot,
            }),
        )
        .await
}

#[cfg(feature = "runtime-adapter")]
impl MobSessionRuntimePostStopCleanupHandle {
    pub(super) fn new(
        session_service: Arc<dyn MobSessionService>,
        bridge_session_id: SessionId,
        state: Arc<RuntimeSessionState>,
        runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    ) -> Self {
        Self {
            session_service,
            bridge_session_id,
            state,
            runtime_sessions,
        }
    }

    async fn cleanup(&self, under_turn_boundary: bool) -> Result<(), CoreExecutorError> {
        let _turn_finalization_guard = if under_turn_boundary {
            None
        } else {
            Some(
                self.session_service
                    .acquire_runtime_turn_finalization_guard(&self.bridge_session_id)
                    .await
                    .map_err(CoreExecutorError::apply_failed_from_session_error)?,
            )
        };
        let _operation_guard = self.state.operation_guard().await;
        let _ = self.state.mark_cleanup_scheduled();
        // This callback is retained by the machine's exact attachment and is
        // invoked only inside that attachment's unregister saga. The machine
        // prevents replacement from crossing the retained mutation fence,
        // while the service boundary above prevents actor replacement. The
        // sidecar map is therefore only a compare-and-remove index, never the
        // authority that decides whether this exact actor may be discarded.
        let actor_witness = self.state.actor_witness().ok_or_else(|| {
            CoreExecutorError::control_failed_runtime(format!(
                "runtime sidecar for session '{}' has no exact live actor witness",
                self.bridge_session_id
            ))
        })?;
        let discard = self
            .session_service
            .discard_live_session_actor_under_runtime_turn_boundary(&actor_witness)
            .await;
        if let Err(error) = discard
            && !matches!(error, SessionError::NotFound { .. })
        {
            return Err(CoreExecutorError::control_failed_runtime(error.to_string()));
        }

        let mut runtime_sessions = self.runtime_sessions.write().await;
        if runtime_sessions
            .get(&self.bridge_session_id)
            .is_some_and(|current| same_runtime_attachment(current, &self.state))
        {
            runtime_sessions.remove(&self.bridge_session_id);
        }
        drop(runtime_sessions);
        self.state.clear_queued_turns().await;
        self.state.mark_retired();
        Ok(())
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutorPostStopCleanupHandle for MobSessionRuntimePostStopCleanupHandle {
    async fn cleanup_after_runtime_stop_terminalized(&self) -> Result<(), CoreExecutorError> {
        self.cleanup(false).await
    }

    async fn cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
    ) -> Result<(), CoreExecutorError> {
        self.cleanup(true).await
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutorPublicationHandle for MobSessionTerminalPublicationHandle {
    async fn publish_interaction_terminals(
        &self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        self.session_service
            .publish_interaction_terminals(&self.bridge_session_id, events)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutorInterruptHandle for MobSessionServiceInterruptHandle {
    async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.session_service
            .interrupt_with_machine_authority(
                &self.bridge_session_id,
                self.runtime_adapter.session_control_authority(),
            )
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }
}

#[cfg(feature = "runtime-adapter")]
fn pending_system_context_appends_for_runtime_executor(
    appends: &[meerkat_core::lifecycle::run_primitive::ConversationContextAppend],
) -> Vec<PendingSystemContextAppend> {
    #[cfg(not(target_arch = "wasm32"))]
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    #[cfg(target_arch = "wasm32")]
    let accepted_at = meerkat_core::time_compat::UNIX_EPOCH;
    appends
        .iter()
        .map(|append| PendingSystemContextAppend {
            content: append.content.clone(),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            // Durable keyed conversation context append — not a transient steer.
            source_kind: meerkat_core::session::SystemContextSource::Normal,
            accepted_at,
            peer_response_terminal: None,
        })
        .collect()
}

#[cfg(feature = "runtime-adapter")]
fn runtime_llm_reconfigure_request_from_primitive(
    primitive: &RunPrimitive,
) -> Option<meerkat_runtime::SessionLlmReconfigureRequest> {
    let metadata = primitive.turn_metadata()?;
    if metadata.model.is_none()
        && metadata.provider.is_none()
        && metadata.self_hosted_server_id.is_none()
        && metadata.provider_params.is_none()
        && metadata.auth_binding.is_none()
    {
        return None;
    }
    Some(meerkat_runtime::SessionLlmReconfigureRequest {
        model: metadata
            .model
            .as_ref()
            .map(|model| model.as_str().to_string()),
        provider: metadata
            .provider
            .as_ref()
            .map(|provider| provider.as_str().to_string()),
        self_hosted_server_id: metadata.self_hosted_server_id.clone(),
        provider_params: metadata.provider_params.clone(),
        auth_binding: metadata.auth_binding.clone(),
    })
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutor for MobSessionRuntimeExecutor {
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(Arc::new(MobSessionRuntimeBoundaryHandle {
            session_service: Arc::clone(&self.session_service),
            runtime_adapter: Arc::clone(&self.runtime_adapter),
            bridge_session_id: self.bridge_session_id.clone(),
        }))
    }

    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        Some(Arc::new(MobSessionServiceInterruptHandle {
            session_service: Arc::clone(&self.session_service),
            runtime_adapter: Arc::clone(&self.runtime_adapter),
            bridge_session_id: self.bridge_session_id.clone(),
        }))
    }

    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        Some(Arc::new(MobSessionTerminalPublicationHandle {
            session_service: Arc::clone(&self.session_service),
            bridge_session_id: self.bridge_session_id.clone(),
        }))
    }

    fn machine_managed_post_stop_unregister(&self) -> bool {
        true
    }

    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        Some(Arc::new(MobSessionRuntimePostStopCleanupHandle {
            session_service: Arc::clone(&self.session_service),
            bridge_session_id: self.bridge_session_id.clone(),
            state: Arc::clone(&self.state),
            runtime_sessions: Arc::clone(&self.runtime_sessions),
        }))
    }

    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>> {
        Some(Arc::new(MobSessionTurnFinalizationBoundaryHandle {
            session_service: Arc::clone(&self.session_service),
            bridge_session_id: self.bridge_session_id.clone(),
        }))
    }

    async fn apply(
        &mut self,
        run_id: CoreRunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
            return Err(CoreExecutorError::apply_failed_primitive_rejected(
                reason.to_string(),
            ));
        }

        // Context-only staged primitives may land directly as runtime
        // system-context appends, but terminal peer responses carry a typed
        // apply intent that requires a requester reaction turn.
        if primitive.is_context_only_apply_without_turn() {
            let RunPrimitive::StagedInput(staged) = &primitive else {
                return Err(CoreExecutorError::apply_failed_primitive_rejected(
                    "context-only apply without turn requires a staged input primitive",
                ));
            };
            return self
                .session_service
                .apply_runtime_context_appends_with_boundary(
                    &self.bridge_session_id,
                    run_id,
                    pending_system_context_appends_for_runtime_executor(&staged.context_appends),
                    staged.boundary,
                    staged.contributing_input_ids.clone(),
                )
                .await
                .map_err(|err| CoreExecutorError::apply_failed_runtime_context(err.to_string()));
        }

        let contributing_input_ids = primitive.contributing_input_ids().to_vec();
        let pre_turn_context_appends = match &primitive {
            RunPrimitive::StagedInput(staged)
                if primitive.is_peer_response_terminal_context_and_run() =>
            {
                pending_system_context_appends_for_runtime_executor(&staged.context_appends)
            }
            _ => Vec::new(),
        };
        let queued_context = self
            .state
            .take_turn_context_for_inputs(&contributing_input_ids)
            .await;
        let (event_tx, mut llm_identity_application) = match queued_context {
            Some(context) => context.into_executor_parts(),
            None => (None, PendingMemberTurnLlmIdentityApplication::default()),
        };
        // The runtime has already resolved handling_mode routing (Queue vs
        // Steer) before the executor fires. The session-service start_turn
        // path only supports Queue — Steer semantics are realized by the
        // runtime ingress, not by the executor. render_metadata is
        // runtime-owned and must not leak into the session-service path;
        // strip it from turn_metadata and pass None at the request level.
        let executor_turn_metadata = primitive.turn_metadata().map(|meta| {
            let mut m = meta.clone();
            m.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);
            m.render_metadata = None;
            m
        });
        let mut req = StartTurnRequest {
            injected_context: Vec::new(),
            prompt: primitive.extract_content_input(),
            system_prompt: None,
            event_tx,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                meerkat_core::types::HandlingMode::Queue,
                primitive
                    .turn_metadata()
                    .and_then(|meta| meta.turn_tool_overlay.clone()),
                pre_turn_context_appends,
                executor_turn_metadata,
            )
            .with_typed_turn_appends(primitive.typed_turn_appends()),
        };
        meerkat::surface::inject_workgraph_attention_turn_overlay(
            self.session_service.as_ref(),
            self.workgraph_service.as_ref(),
            &self.bridge_session_id,
            &mut req,
        )
        .await
        .map_err(|error| CoreExecutorError::apply_failed_primitive_rejected(error.to_string()))?;

        // A per-turn LLM selection belongs to this exact serialized runtime
        // primitive, not to admission-time mutation of shared session state.
        // The applied signal is the canonical resolver's actual identity and
        // remains valid even if the subsequent turn or finalization fails.
        if let Some(request) = runtime_llm_reconfigure_request_from_primitive(&primitive) {
            let report = self
                .runtime_adapter
                .reconfigure_session_llm_identity_under_turn_finalization_boundary(
                    &self.bridge_session_id,
                    request,
                )
                .await
                .map_err(|error| {
                    llm_identity_application.resolve_internal_failure(format!(
                        "failed to reconfigure member session '{}' LLM identity for run {run_id}: {error}",
                        self.bridge_session_id
                    ));
                    CoreExecutorError::apply_failed_runtime_turn(format!(
                        "failed to reconfigure member session '{}' LLM identity for run {run_id}: {error}",
                        self.bridge_session_id
                    ))
                })?;
            llm_identity_application.resolve_success(Some(report.new_identity));
        } else {
            llm_identity_application.resolve_success(None);
        }

        self.session_service
            .apply_runtime_turn(
                &self.bridge_session_id,
                run_id,
                req,
                match &primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => RunApplyBoundary::Immediate,
                },
                contributing_input_ids,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        session_snapshot: &[u8],
    ) -> Result<(), CoreExecutorError> {
        self.session_service
            .checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
                &self.bridge_session_id,
                session_snapshot,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn reconcile_committed_compaction_projections(
        &mut self,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), CoreExecutorError> {
        self.session_service
            .reconcile_runtime_compaction_projections(&self.bridge_session_id, intents.to_vec())
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.session_service
            .abort_uncommitted_compaction_projections(&self.bridge_session_id)
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn abort_rejected_run_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.session_service
            .abort_rejected_runtime_run_projections(&self.bridge_session_id)
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        MobSessionTerminalPublicationHandle {
            session_service: Arc::clone(&self.session_service),
            bridge_session_id: self.bridge_session_id.clone(),
        }
        .publish_interaction_terminals(events)
        .await
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.session_service
            .cancel_current_after_boundary_with_machine_authority(
                &self.bridge_session_id,
                self.runtime_adapter.session_control_authority(),
            )
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized(&mut self) -> Result<(), CoreExecutorError> {
        MobSessionRuntimePostStopCleanupHandle {
            session_service: Arc::clone(&self.session_service),
            bridge_session_id: self.bridge_session_id.clone(),
            state: Arc::clone(&self.state),
            runtime_sessions: Arc::clone(&self.runtime_sessions),
        }
        .cleanup_after_runtime_stop_terminalized()
        .await
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobProvisioner for SessionBackend {
    fn supports_member_turn_llm_reconfigure(&self, member_ref: &MemberRef) -> bool {
        matches!(member_ref, MemberRef::Session { .. })
            && self.session_service.supports_runtime_turn_apply()
            && self
                .runtime_adapter
                .as_ref()
                .is_some_and(|adapter| adapter.has_session_llm_reconfigure_host())
    }

    async fn provision_member(
        &self,
        mut req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError> {
        // Every cancellation-sensitive resource below is represented by an
        // exact RAII lease. Keep the saga in the caller task so a successful
        // receipt can never be detached and dropped without publishing its
        // rollback capability to the owning PendingProvision.
        let backend = self;
        let mut session_origin = req.session_origin;
        let missing_live_revival =
            req.runtime_revival_intent == RuntimeRevivalIntent::MissingLiveMaterialization;
        let local_materialization_mode = if missing_live_revival {
            meerkat_runtime::LocalSessionMaterializationMode::MissingLiveRevival
        } else {
            meerkat_runtime::LocalSessionMaterializationMode::Ordinary
        };
        tracing::debug!(
            binding = ?req.binding,
            peer_name = %req.peer_name,
            "SessionBackend::provision_member start"
        );
        let admitted_bridge_session_id = req
            .create_session
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone());
        // Prepare local session resources for the factory. The authoritative
        // mob binding is emitted later by the routed
        // RequestRuntimeBinding -> PrepareBindings path, after MobMachine has
        // committed the member-owned AgentRuntimeId and fence.
        let mut prepared_ops_binding: Option<(SessionId, Arc<dyn OpsLifecycleRegistry>)> = None;
        let mut prepared_materialization: Option<PreparedSessionMaterialization> = None;
        let mut attached_actor_recovery: Option<(
            PreparedAttachedSessionActorRecovery,
            Arc<RuntimeSessionState>,
        )> = None;
        let mut recovered_attached_state: Option<Arc<RuntimeSessionState>> = None;
        let mut archived_resume_authorization = None;
        let mut reviving_retired_session = false;
        let mut materialization_turn_boundary: Option<RuntimeTurnFinalizationBoundaryLease> = None;
        let actor_witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
        let pre_registered_bridge_session_id = if let Some(adapter) = &backend.runtime_adapter {
            if req.create_session.build.is_none() {
                req.create_session.build =
                    Some(meerkat_core::service::SessionBuildOptions::default());
            }
            let member_bridge_session_id = req
                .create_session
                .build
                .as_ref()
                .and_then(|b| b.resume_session.as_ref())
                .map(|s| s.id().clone())
                .unwrap_or_else(|| {
                    let id = SessionId::new();
                    let session = meerkat_core::session::Session::with_id(id.clone());
                    if let Some(ref mut build) = req.create_session.build {
                        build.resume_session = Some(session);
                    }
                    id
                });
            // Global order is B -> M -> R. Acquire the stable service boundary
            // before asking the machine to mint the exact materialization
            // claim, and retain it through actor creation, attachment commit,
            // receipt construction, or exact rollback.
            materialization_turn_boundary = Some(
                RuntimeTurnFinalizationBoundaryLease::acquire(
                    &backend.session_service,
                    &member_bridge_session_id,
                )
                .await?,
            );
            // MobMachine::Spawn routes the authoritative member runtime id
            // and fence token after the bridge session exists. This call only
            // needs the session-local handle bundle for AgentFactory.
            tracing::debug!(
                bridge_session_id = %member_bridge_session_id,
                "SessionBackend::provision_member preparing local session bindings"
            );
            let current_attachment = if missing_live_revival {
                adapter
                    .current_executor_attachment_witness(&member_bridge_session_id)
                    .await
            } else {
                None
            };
            if let Some(witness) = current_attachment {
                let state = backend
                    .runtime_sessions
                    .read()
                    .await
                    .get(&member_bridge_session_id)
                    .cloned()
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "missing-live actor recovery for '{member_bridge_session_id}' found an exact machine attachment without its attachment-local mob sidecar; explicit resume is required"
                        ))
                    })?;
                if state.witness() != &witness
                    || !state.attachment_is_active(&witness)
                    || state.cleanup_scheduled()
                    || !state.queued_turn_owner.is_owned_by(&witness)
                {
                    return Err(MobError::Internal(format!(
                        "missing-live actor recovery for '{member_bridge_session_id}' found stale attachment-local mob state; explicit resume is required"
                    )));
                }
                let prepared = adapter
                    .prepare_attached_session_actor_recovery(&witness)
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "prepare exact attached actor recovery for '{member_bridge_session_id}' failed: {error}"
                        ))
                    })?;
                let bindings = prepared.bindings_clone();
                let ops_registry = Arc::clone(bindings.ops_lifecycle());
                if let Some(ref mut build) = req.create_session.build {
                    build.runtime_build_mode =
                        meerkat_core::runtime_epoch::RuntimeBuildMode::SessionOwned(bindings);
                }
                prepared_ops_binding = Some((member_bridge_session_id.clone(), ops_registry));
                attached_actor_recovery = Some((prepared, state));
                tracing::debug!(
                    bridge_session_id = %member_bridge_session_id,
                    "SessionBackend::provision_member prepared actor-only recovery for exact attachment"
                );
            } else {
                #[cfg(target_arch = "wasm32")]
                let mut prepared = {
                    let adapter = Arc::clone(adapter);
                    let bridge_session_id = member_bridge_session_id.clone();
                    let (reply_tx, reply_rx) = oneshot::channel();
                    tokio::spawn(async move {
                        let result = adapter
                            .prepare_local_session_materialization_with_mode(
                                bridge_session_id,
                                local_materialization_mode,
                            )
                            .await;
                        let _ = reply_tx.send(result);
                    });
                    reply_rx.await.map_err(|_| {
                        MobError::Internal(
                            "prepare local session bindings task was canceled".to_string(),
                        )
                    })?
                }
                .map_err(|e| {
                    MobError::Internal(format!("prepare local session bindings failed: {e}"))
                })?;
                #[cfg(not(target_arch = "wasm32"))]
                let mut prepared = adapter
                    .prepare_local_session_materialization_with_mode(
                        member_bridge_session_id.clone(),
                        local_materialization_mode,
                    )
                    .await
                    .map_err(|e| {
                        MobError::Internal(format!(
                            "prepare local session materialization failed: {e}"
                        ))
                    })?;
                install_prepared_mob_session_executor_handles(
                    Arc::clone(&backend.session_service),
                    Arc::clone(adapter),
                    &prepared,
                    actor_witness_slot.clone(),
                )
                .await
                .map_err(|error| {
                    MobError::Internal(format!(
                        "install prepared session cleanup handles failed: {error}"
                    ))
                })?;
                tracing::debug!(
                    bridge_session_id = %member_bridge_session_id,
                    "SessionBackend::provision_member prepared exact local session materialization"
                );
                let bindings = prepared.bindings_clone();
                let ops_registry = Arc::clone(bindings.ops_lifecycle());
                if session_origin == ProvisionSessionOrigin::ResumedDurable
                    && adapter
                        .meerkat_machine_archive_snapshot(&member_bridge_session_id)
                        .await
                        .is_some_and(|snapshot| {
                            snapshot.control.phase == meerkat_runtime::RuntimeState::Retired
                        })
                {
                    reviving_retired_session = true;
                    archived_resume_authorization =
                        Some(prepared.archived_resume_authorization().map_err(|error| {
                            MobError::Internal(format!(
                                "prepare exact archived-resume authorization failed: {error}"
                            ))
                        })?);
                }
                if let Some(ref mut build) = req.create_session.build {
                    build.runtime_build_mode =
                        meerkat_core::runtime_epoch::RuntimeBuildMode::SessionOwned(bindings);
                }
                prepared_ops_binding = Some((member_bridge_session_id.clone(), ops_registry));
                prepared_materialization = Some(prepared);
            }
            tracing::debug!(
                bridge_session_id = %member_bridge_session_id,
                "SessionBackend::provision_member stamping eager turn metadata"
            );
            stamp_eager_session_owned_initial_turn_metadata(&mut req.create_session);
            tracing::debug!(
                bridge_session_id = %member_bridge_session_id,
                "SessionBackend::provision_member stamped eager turn metadata"
            );
            Some(member_bridge_session_id)
        } else {
            None
        };
        let mut actor_transaction = if let Some(pre_registered_session_id) =
            pre_registered_bridge_session_id.as_ref()
        {
            if attached_actor_recovery.is_some() {
                None
            } else {
                let prepared = prepared_materialization.take().ok_or_else(|| {
                    MobError::Internal(format!(
                        "runtime-backed provision for '{pre_registered_session_id}' lost Prepared before actor creation"
                    ))
                })?;
                let boundary = materialization_turn_boundary.take().ok_or_else(|| {
                    MobError::Internal(format!(
                        "runtime-backed provision for '{pre_registered_session_id}' lost B before actor creation"
                    ))
                })?;
                Some(PreparedServiceActorTransaction::new(
                    pre_registered_session_id.clone(),
                    Arc::clone(&backend.session_service),
                    prepared,
                    boundary,
                    actor_witness_slot.clone(),
                )?)
            }
        } else {
            None
        };
        tracing::debug!("SessionBackend::provision_member creating bridge session");
        let create_authorization = if reviving_retired_session {
            Some(archived_resume_authorization.take().ok_or_else(|| {
                MobError::Internal(
                    "retired durable session revival lost its exact prepared authorization".into(),
                )
            })?)
        } else {
            None
        };
        let create_result: Result<meerkat_core::RunResult, MobError> = if let Some((
            prepared,
            state,
        )) =
            attached_actor_recovery.take()
        {
            if create_authorization.is_some() {
                Err(MobError::Internal(format!(
                    "actor-only recovery for '{}' unexpectedly carried archived-resume authority",
                    prepared.witness().session_id()
                )))
            } else {
                let recovery_session_id = prepared.witness().session_id().clone();
                let boundary = materialization_turn_boundary.take().ok_or_else(|| {
                        MobError::Internal(format!(
                            "actor-only recovery for '{recovery_session_id}' lost B before actor creation"
                        ))
                    })?;
                let created = create_attached_session_actor_recovery_owned(
                    recovery_session_id,
                    Arc::clone(&backend.session_service),
                    prepared,
                    boundary,
                    Arc::clone(&state),
                    actor_witness_slot.clone(),
                    req.create_session,
                )
                .await?;
                recovered_attached_state = Some(state);
                Ok(created)
            }
        } else if let Some(transaction) = actor_transaction.take() {
            match transaction
                .create_owned(req.create_session, create_authorization)
                .await
            {
                Ok((created, transaction)) => {
                    actor_transaction = Some(transaction);
                    Ok(created)
                }
                Err(error) => Err(error),
            }
        } else if create_authorization.is_some() {
            Err(MobError::Internal(
                "retired durable session revival lost its actor transaction".into(),
            ))
        } else {
            backend
                .session_service
                .create_session(req.create_session)
                .await
                .map_err(MobError::from)
        };
        let created = match create_result {
            Ok(created) => created,
            Err(error) => return Err(error),
        };
        tracing::debug!(
            bridge_session_id = %created.session_id,
            "SessionBackend::provision_member created session service session"
        );
        let created_bridge_session_id = created.session_id.clone();
        if admitted_bridge_session_id
            .as_ref()
            .is_some_and(|admitted| admitted != &created_bridge_session_id)
            || pre_registered_bridge_session_id
                .as_ref()
                .is_some_and(|prepared| prepared != &created_bridge_session_id)
        {
            return Err(MobError::Internal(format!(
                "session service returned unexpected bridge session '{created_bridge_session_id}'; refusing SessionId-wide cleanup of the unexpected identity"
            )));
        }
        let finalize_result = async {
        // If no admission id was supplied, clean up stale local pre-registration
        // defensively. Normal mob spawn paths now admit a concrete session id
        // before provisioning starts.
        if let (Some(adapter), Some(pre_id)) =
            (&backend.runtime_adapter, &pre_registered_bridge_session_id)
        {
            if reviving_retired_session {
                // Explicit revival owns the only path across both absorbing
                // lifecycle boundaries. Prove and promote the archived
                // document while the runtime is still durably Retired, then
                // reset Retired -> Idle before generic executor attachment.
                // Retired must remain non-registrable on every ordinary
                // ensure-session path.
                let prepared = actor_transaction
                    .as_ref()
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "revived session '{created_bridge_session_id}' lost its actor transaction before durable promotion"
                        ))
                    })?
                    .prepared()
                    .map_err(|error| {
                    MobError::Internal(format!(
                        "revived session '{created_bridge_session_id}' lost its exact prepared materialization before durable promotion: {error}"
                    ))
                })?;
                let commit_lease = prepared
                    .acquire_archived_resume_commit_lease()
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to acquire exact archived-resume commit lease for '{created_bridge_session_id}': {error}"
                        ))
                    })?;
                let mut promoted_lease = backend.session_service
                    .promote_revivable_retired_session(
                        &created_bridge_session_id,
                        commit_lease,
                    )
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to promote revived durable session document '{created_bridge_session_id}': {error}"
                        ))
                    })?;
                promoted_lease
                    .reset_retired_runtime()
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to promote revived durable session '{created_bridge_session_id}' to idle: {error}"
                        ))
                    })?;
                session_origin = ProvisionSessionOrigin::RevivedRetired;
            }
            tracing::debug!(
                bridge_session_id = %created_bridge_session_id,
                "SessionBackend::provision_member retained exact prepared materialization"
            );
        }
        match (
            req.owner_bridge_session_id,
            req.ops_registry,
            req.generated_self_owned_operation_owner,
        ) {
            (Some(owner_bridge_session_id), Some(registry), None) => {
                tracing::debug!(
                    bridge_session_id = %created_bridge_session_id,
                    "SessionBackend::provision_member binding owner session registry"
                );
                backend.ops_adapter.bind_session_registry(
                    created_bridge_session_id.clone(),
                    owner_bridge_session_id,
                    registry,
                )?;
                tracing::debug!(
                    bridge_session_id = %created_bridge_session_id,
                    "SessionBackend::provision_member bound owner session registry"
                );
            }
            (None, None, Some(generated_owner_session_id)) => {
                let Some((prepared_owner_session_id, registry)) = prepared_ops_binding else {
                    return Err(MobError::Internal(
                        "mob provisioner received generated operation owner authority without prepared generated bindings"
                            .into(),
                    ));
                };
                if generated_owner_session_id != created_bridge_session_id
                    || prepared_owner_session_id != created_bridge_session_id
                {
                    return Err(MobError::Internal(format!(
                        "generated pending spawn operation owner '{generated_owner_session_id}' did not match created bridge session '{created_bridge_session_id}'"
                    )));
                }
                tracing::debug!(
                    bridge_session_id = %created_bridge_session_id,
                    "SessionBackend::provision_member binding generated owner session registry"
                );
                backend.ops_adapter.bind_session_registry(
                    created_bridge_session_id.clone(),
                    generated_owner_session_id,
                    registry,
                )?;
                tracing::debug!(
                    bridge_session_id = %created_bridge_session_id,
                    "SessionBackend::provision_member bound generated owner session registry"
                );
            }
            (None, None, None) => {
                return Err(MobError::Internal(
                    "session member operation requires generated owner binding".into(),
                ));
            }
            _ => {
                return Err(MobError::Internal(
                    "mob provisioner received partial ops owner binding".into(),
                ));
            }
        }
        tracing::debug!(
            bridge_session_id = %created_bridge_session_id,
            peer_name = %req.peer_name,
            "SessionBackend::provision_member marking member provisioned"
        );
        let prepared_operation = backend
            .ops_adapter
            .prepare_member_provision_operation(&created_bridge_session_id, &req.peer_name)
            .await?;
        let exact_operation_id = prepared_operation.operation_id().clone();
        tracing::debug!(
            bridge_session_id = %created_bridge_session_id,
            operation_id = %exact_operation_id,
            "SessionBackend::provision_member prepared member provision operation"
        );
        let mut rollback_authority = None;
        let committed_operation;
        if let Some(adapter) = backend.runtime_adapter.as_ref() {
            if let Some(committed_state) = recovered_attached_state.take() {
                // Actor-only recovery preserves the exact serving attachment;
                // the existing operation is a validated Running replay and is
                // committed without any second attachment publication.
                committed_operation = prepared_operation.commit()?;
                if session_origin != ProvisionSessionOrigin::Fresh {
                    rollback_authority =
                        Some(ResumedMemberRollbackAuthority::new(committed_state));
                }
            } else {
                // This is the final async transaction before receipt publication.
                // Transfer Prepared+B by value before startup can mint Pending/M;
                // caller cancellation can therefore never split their cleanup.
                let transaction = actor_transaction.take().ok_or_else(|| {
                    MobError::Internal(format!(
                        "runtime-backed provision for '{created_bridge_session_id}' lost its actor transaction at owned attachment transfer"
                    ))
                })?;
                let (committed_state, exact_committed_operation, transaction) = transaction
                    .prepare_and_commit_runtime_attachment_and_operation_owned(
                        Arc::clone(adapter),
                        backend.workgraph_service.clone(),
                        Arc::clone(&backend.runtime_sessions),
                        missing_live_revival,
                        prepared_operation,
                    )
                    .await?;
                actor_transaction = Some(transaction);
                committed_operation = exact_committed_operation;
                if session_origin != ProvisionSessionOrigin::Fresh {
                    rollback_authority =
                        Some(ResumedMemberRollbackAuthority::new(committed_state));
                }
            }
        } else {
            committed_operation = prepared_operation.commit()?;
        }
        tracing::debug!(
            bridge_session_id = %created_bridge_session_id,
            "SessionBackend::provision_member atomically committed attachment and operation"
        );
        let operation_id = committed_operation.disarm()?;
        if let Some(transaction) = actor_transaction.as_mut() {
            transaction.commit();
        }
        Ok(MemberSpawnReceipt {
            member_ref: MemberRef::from_bridge_session_id(created_bridge_session_id),
            operation_id,
            session_origin,
            rollback_authority,
            materialized_ack: None,
            failed_restore_peer_ids: Vec::new(),
        })
        }
        .await;
        if let Err(error) = finalize_result {
            if let Some(transaction) = actor_transaction.take()
                && let Err(cleanup_error) = transaction.abort().await
            {
                return Err(MobError::Internal(format!(
                    "provision failed: {error}; exact actor/materialization cleanup failed: {cleanup_error}"
                )));
            }
            return Err(error);
        }
        finalize_result
    }

    async fn abort_member_provision(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError> {
        let bridge_session_id = Self::require_session(member_ref, "abort provision for")?;
        match self
            .ops_adapter
            .operation_status_with_terminality(&bridge_session_id, operation_id)?
        {
            Some((OperationStatus::Provisioning, _)) => {
                match self
                    .archive_with_authority_then_unregister(&bridge_session_id)
                    .await
                {
                    Ok(_) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                self.ops_adapter
                    .abort_member_provision(
                        &bridge_session_id,
                        operation_id,
                        Some(reason.to_string()),
                    )
                    .await
            }
            Some((OperationStatus::Absent, _)) | None => {
                match self
                    .archive_with_authority_then_unregister(&bridge_session_id)
                    .await
                {
                    Ok(_) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            }
            Some((_status, true)) => {
                match self
                    .archive_with_authority_then_unregister(&bridge_session_id)
                    .await
                {
                    Ok(_) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            }
            Some((_status, false)) => self.retire_member(member_ref).await.map(|_| ()),
        }
    }

    async fn capture_resumed_member_rollback_authority(
        &self,
        member_ref: &MemberRef,
    ) -> Result<ResumedMemberRollbackAuthority, MobError> {
        let session_id =
            Self::require_session(member_ref, "capture failed-resume rollback authority for")?;
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "cannot capture resumed rollback authority for '{session_id}' without runtime authority"
            ))
        })?;
        let state = self
            .runtime_sessions
            .read()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "resumed session '{session_id}' has no attachment-local rollback sidecar"
                ))
            })?;
        let current = adapter
            .current_executor_attachment_witness(&session_id)
            .await;
        if current.as_ref() != Some(state.witness()) || !state.attachment_is_active(state.witness())
        {
            return Err(MobError::Internal(format!(
                "resumed session '{session_id}' changed before exact rollback authority capture"
            )));
        }
        Ok(ResumedMemberRollbackAuthority::new(state))
    }

    async fn restore_resumed_member(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        original_origin: ProvisionSessionOrigin,
        rollback_authority: &ResumedMemberRollbackAuthority,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "restore failed resume for")?;
        let expected_state = rollback_authority.state().ok_or_else(|| {
            MobError::Internal(format!(
                "resumed session '{session_id}' received non-runtime rollback authority"
            ))
        })?;
        if expected_state.witness().session_id() != &session_id {
            return Err(MobError::Internal(format!(
                "rollback authority for '{}' cannot restore resumed session '{session_id}'",
                expected_state.witness().session_id()
            )));
        }
        let mut boundary =
            RuntimeTurnFinalizationBoundaryLease::acquire(&self.session_service, &session_id)
                .await?;
        self.restore_failed_resume_before_receipt(
            &session_id,
            original_origin == ProvisionSessionOrigin::RevivedRetired,
            FailedResumeRuntimeAuthority::ExactAttachment(expected_state),
            &mut boundary,
        )
        .await?;

        if matches!(
            self.ops_adapter
                .operation_status_with_terminality(&session_id, operation_id)?,
            Some((OperationStatus::Provisioning, false))
        ) {
            self.ops_adapter
                .abort_member_provision(
                    &session_id,
                    operation_id,
                    Some("resume provisioning rolled back without archive".to_string()),
                )
                .await?;
        }
        Ok(())
    }

    async fn retire_member(
        &self,
        member_ref: &MemberRef,
    ) -> Result<crate::machines::mob_machine::MemberSessionDisposal, MobError> {
        tracing::debug!(
            member_ref = ?member_ref,
            "SessionBackend::retire_member start"
        );
        let session_id = Self::require_session(member_ref, "retire")?;
        let disposal = self.disposal.dispose(&session_id).await?;
        self.ops_adapter
            .mark_member_retired_after_disposal(member_ref, disposal)
            .await?;
        Ok(match disposal {
            MemberSessionDisposalVerdict::Archived
            | MemberSessionDisposalVerdict::AlreadyArchived => {
                crate::machines::mob_machine::MemberSessionDisposal::Archived
            }
            MemberSessionDisposalVerdict::RuntimeReleasedOnlyHostOwned => {
                crate::machines::mob_machine::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned
            }
        })
    }

    async fn interrupt_member(
        &self,
        member_ref: &MemberRef,
        _expected_member: Option<&super::bridge_protocol::BridgeMemberIncarnation>,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "interrupt")?;
        if let Some(adapter) = &self.runtime_adapter {
            let expected_state = self.capture_runtime_session_state(&session_id).await;
            if adapter.contains_session(&session_id).await {
                return match LocalMobRuntimeBridge::new(adapter.clone(), session_id.clone())
                    .interrupt_member()
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(error) => {
                        // Preserve bridge sidecar alignment when registration
                        // changed between contains_session and interrupt.
                        if !adapter.contains_session(&session_id).await {
                            self.remove_runtime_session_state(&session_id, expected_state.as_ref())
                                .await;
                        }
                        Err(MobError::Internal(format!(
                            "runtime-backed interrupt must resolve through MeerkatMachine for '{session_id}': {error}"
                        )))
                    }
                };
            }

            // Runtime-backed members must be interrupted through runtime
            // adapter registration truth, not direct session-service fallback.
            self.remove_runtime_session_state(&session_id, expected_state.as_ref())
                .await;
            return Err(MobError::Internal(format!(
                "runtime-backed interrupt requested for unregistered runtime session '{session_id}'"
            )));
        }
        self.session_service
            .cancel_after_boundary(&session_id)
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })?;
        Ok(())
    }

    async fn hard_cancel_member(
        &self,
        member_ref: &MemberRef,
        reason: &str,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "hard cancel")?;
        if let Some(adapter) = &self.runtime_adapter {
            let expected_state = self.capture_runtime_session_state(&session_id).await;
            if adapter.contains_session(&session_id).await {
                return adapter
                    .hard_cancel_current_run(&session_id, reason)
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "runtime-backed hard cancel must resolve through MeerkatMachine for '{session_id}': {error}"
                        ))
                    });
            }

            self.remove_runtime_session_state(&session_id, expected_state.as_ref())
                .await;
            return Err(MobError::Internal(format!(
                "runtime-backed hard cancel requested for unregistered runtime session '{session_id}'"
            )));
        }
        Err(MobError::Internal(format!(
            "mob session hard cancel for '{session_id}' requires MeerkatMachine runtime authority"
        )))
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "start turn")?;
        if self.runtime_adapter.is_some() {
            self.ops_adapter
                .report_member_progress(member_ref, "turn dispatched")
                .await?;
            let input = Self::runtime_input_from_turn_request(&req);
            return self
                .execute_runtime_input(&session_id, input, req.event_tx)
                .await;
        }

        self.session_service
            .start_turn(&session_id, req)
            .await
            .map(|_| ())
            .map_err(|error| session_turn_error_to_mob_error(&session_id, error))
    }

    async fn admit_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "admit turn")?;
        if self.runtime_adapter.is_some() {
            tracing::debug!(
                session_id = %session_id,
                member_ref = ?member_ref,
                "SessionBackend::admit_turn reporting member progress"
            );
            self.ops_adapter
                .report_member_progress(member_ref, "turn dispatched")
                .await?;
            tracing::debug!(
                session_id = %session_id,
                member_ref = ?member_ref,
                "SessionBackend::admit_turn reported member progress"
            );
            tracing::debug!(
                session_id = %session_id,
                "SessionBackend::admit_turn building runtime input"
            );
            let input = Self::runtime_input_from_turn_request(&req);
            tracing::debug!(
                session_id = %session_id,
                input_id = %input.id(),
                "SessionBackend::admit_turn admitting runtime input"
            );
            return self
                .admit_runtime_input(&session_id, input, req.event_tx, None, None)
                .await;
        }

        let session_service = self.session_service.clone();
        let task_session_id = session_id.clone();
        let mut task = tokio::spawn(async move {
            session_service
                .start_turn(&task_session_id, req)
                .await
                .map(|_| ())
                .map_err(|error| session_turn_error_to_mob_error(&task_session_id, error))
        });

        tokio::select! {
            result = &mut task => {
                result.map_err(|error| {
                    MobError::Internal(format!("turn admission task failed: {error}"))
                })?
            }
            () = tokio::task::yield_now() => Ok(()),
        }
    }

    async fn admit_tracked_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
        completion_tx: tokio::sync::oneshot::Sender<Result<(), MobError>>,
        llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "admit tracked turn")?;
        if self.runtime_adapter.is_some() {
            self.ops_adapter
                .report_member_progress(member_ref, "turn dispatched")
                .await?;
            let input = Self::runtime_input_from_turn_request(&req);
            return self
                .admit_runtime_input(
                    &session_id,
                    input,
                    req.event_tx,
                    Some(completion_tx),
                    llm_identity_applied_tx,
                )
                .await;
        }

        if let Some(llm_identity_applied_tx) = llm_identity_applied_tx {
            let _ = llm_identity_applied_tx.send(Ok(None));
        }
        let session_service = self.session_service.clone();
        tokio::spawn(async move {
            let result = session_service
                .start_turn(&session_id, req)
                .await
                .map(|_| ())
                .map_err(|error| session_turn_error_to_mob_error(&session_id, error));
            let _ = completion_tx.send(result);
        });
        Ok(())
    }

    async fn admit_turn_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "admit turn")?;
        if self.runtime_adapter.is_some() {
            tracing::debug!(
                session_id = %session_id,
                member_ref = ?member_ref,
                operation_id = %operation_id,
                "SessionBackend::admit_turn_for_operation reporting member progress"
            );
            #[cfg(target_arch = "wasm32")]
            {
                tracing::debug!(
                    session_id = %session_id,
                    member_ref = ?member_ref,
                    operation_id = %operation_id,
                    "SessionBackend::admit_turn_for_operation skipping progress report on wasm"
                );
            }
            #[cfg(not(target_arch = "wasm32"))]
            self.ops_adapter.report_member_progress_for_operation(
                member_ref,
                operation_id,
                "turn dispatched",
            )?;
            tracing::debug!(
                session_id = %session_id,
                member_ref = ?member_ref,
                operation_id = %operation_id,
                "SessionBackend::admit_turn_for_operation reported member progress"
            );
            let input = Self::runtime_input_from_turn_request(&req);
            tracing::debug!(
                session_id = %session_id,
                input_id = %input.id(),
                operation_id = %operation_id,
                "SessionBackend::admit_turn_for_operation admitting runtime input"
            );
            return self
                .admit_runtime_input(&session_id, input, req.event_tx, None, None)
                .await;
        }

        self.admit_turn(member_ref, req).await
    }

    async fn start_flow_step(
        &self,
        member_ref: &MemberRef,
        run_id: &RunId,
        step_id: &crate::StepId,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "start flow step")?;
        if self.runtime_adapter.is_some() {
            let input = meerkat_runtime::mob_adapter::create_flow_step_input(
                step_id.as_str(),
                req.prompt.clone(),
                &run_id.to_string(),
                0,
                Some({
                    // Consume the canonical carrier; flats only backfill
                    // handling/overlay.
                    let mut turn_metadata = req.runtime.turn_metadata.clone().unwrap_or_default();
                    turn_metadata.handling_mode = Some(
                        turn_metadata
                            .handling_mode
                            .unwrap_or(req.runtime.handling_mode),
                    );
                    if turn_metadata.turn_tool_overlay.is_none() {
                        turn_metadata.turn_tool_overlay = req.runtime.turn_tool_overlay.clone();
                    }
                    turn_metadata
                }),
            );
            return self
                .admit_runtime_input(&session_id, input, req.event_tx, None, None)
                .await;
        }

        self.start_turn(member_ref, req).await
    }

    async fn interaction_event_injector(
        &self,
        bridge_session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.session_service
            .interaction_event_injector(bridge_session_id)
            .await
    }

    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError> {
        let bridge_session_id = match member_ref.bridge_session_id() {
            Some(id) => id.clone(),
            None => return Ok(None),
        };
        match self.session_service.read(&bridge_session_id).await {
            Ok(view) => Ok(Some(view.state.is_active)),
            Err(meerkat_core::service::SessionError::NotFound { .. }) => Ok(Some(false)),
            Err(error) => Err(error.into()),
        }
    }

    async fn prepare_member_session_for_explicit_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, MobError> {
        self.retire_exact_attachment_for_explicit_resume(session_id)
            .await
    }

    async fn ensure_runtime_session_state(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let bridge_session_id = Self::require_session(member_ref, "ensure runtime session for")?;
        self.runtime_session_state(&bridge_session_id)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "runtime adapter unavailable while ensuring session state for '{bridge_session_id}'"
                ))
            })?;
        Ok(())
    }

    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        let bridge_session_id = member_ref.bridge_session_id()?;
        self.session_service.comms_runtime(bridge_session_id).await
    }

    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let trusted_peer = if let Some(session_id) = member_ref.bridge_session_id() {
            match self.session_service.comms_runtime(session_id).await {
                Some(runtime) => {
                    if let Some(spec) =
                        Self::trusted_peer_spec_from_runtime(fallback_name, &*runtime)?
                    {
                        spec
                    } else {
                        Self::trusted_peer_spec(fallback_name, fallback_peer_id)?
                    }
                }
                None => Self::trusted_peer_spec(fallback_name, fallback_peer_id)?,
            }
        } else {
            Self::trusted_peer_spec(fallback_name, fallback_peer_id)?
        };
        tracing::debug!(
            fallback_name,
            "SessionBackend::trusted_peer_spec marking member peer ready"
        );
        self.ops_adapter
            .mark_member_peer_ready(member_ref, fallback_name, trusted_peer.clone())
            .await?;
        tracing::debug!(
            fallback_name,
            "SessionBackend::trusted_peer_spec marked member peer ready"
        );
        Ok(trusted_peer)
    }

    async fn trusted_peer_spec_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let trusted_peer = if let Some(session_id) = member_ref.bridge_session_id() {
            match self.session_service.comms_runtime(session_id).await {
                Some(runtime) => {
                    if let Some(spec) =
                        Self::trusted_peer_spec_from_runtime(fallback_name, &*runtime)?
                    {
                        spec
                    } else {
                        Self::trusted_peer_spec(fallback_name, fallback_peer_id)?
                    }
                }
                None => Self::trusted_peer_spec(fallback_name, fallback_peer_id)?,
            }
        } else {
            Self::trusted_peer_spec(fallback_name, fallback_peer_id)?
        };
        tracing::debug!(
            fallback_name,
            operation_id = %operation_id,
            "SessionBackend::trusted_peer_spec_for_operation marking member peer ready"
        );
        self.ops_adapter.mark_member_peer_ready_for_operation(
            member_ref,
            operation_id,
            fallback_name,
            trusted_peer.clone(),
        )?;
        tracing::debug!(
            fallback_name,
            operation_id = %operation_id,
            "SessionBackend::trusted_peer_spec_for_operation marked member peer ready"
        );
        Ok(trusted_peer)
    }

    async fn publish_trusted_peer_spec_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        trusted_peer: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let peer_name = trusted_peer.name.as_str().to_string();
        tracing::debug!(
            peer_name,
            operation_id = %operation_id,
            "SessionBackend::publish_trusted_peer_spec_for_operation publishing exact member peer readiness"
        );
        self.ops_adapter.mark_member_peer_ready_for_operation(
            member_ref,
            operation_id,
            &peer_name,
            trusted_peer,
        )
    }

    async fn active_operation_id_for_member(&self, member_ref: &MemberRef) -> Option<OperationId> {
        let bridge_session_id = member_ref.bridge_session_id()?;
        self.ops_adapter
            .active_operation_id_for_session(bridge_session_id)
            .await
    }

    async fn bind_member_owner_context(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError> {
        let Some(bridge_session_id) = member_ref.bridge_session_id().cloned() else {
            return Err(MobError::Internal(
                "member has no session bridge for canonical ops binding".into(),
            ));
        };
        self.ops_adapter.bind_session_registry(
            bridge_session_id,
            owner_bridge_session_id,
            ops_registry,
        )?;
        Ok(())
    }

    async fn cancel_all_checkpointers(&self) {
        self.session_service.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.session_service.rearm_all_checkpointers().await;
    }
}

#[cfg(feature = "runtime-adapter")]
pub struct MultiBackendProvisioner {
    session: SessionBackend,
    /// Presence carrier: a definition that never declared an external
    /// backend cannot spawn peer-only members (the former `ExternalBackend`
    /// unit wrapper is deleted — the config IS the fact).
    external: Option<ExternalBackendConfig>,
    supervisor_bridge: Arc<super::MobSupervisorBridge>,
    binding_persistence: Option<ProvisionerBindingPersistence>,
}

#[cfg(feature = "runtime-adapter")]
struct ExternalBindingTarget {
    peer_name: String,
    peer_id: String,
    address: String,
    bootstrap_token: Option<super::bridge_protocol::BridgeBootstrapToken>,
    /// Ed25519 signing pubkey of the external process. Propagated into the
    /// supervisor's trust store so inbound signed-envelope replies admit past
    /// ingress `is_trusted` gating.
    pubkey: [u8; 32],
}

#[cfg(feature = "runtime-adapter")]
#[derive(Clone)]
struct ProvisionerBindingPersistence {
    mob_id: crate::MobId,
    runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
}

#[cfg(feature = "runtime-adapter")]
type PeerOnlyBindingParts<'a> = (&'a str, &'a str, Option<&'a str>, [u8; 32]);

#[cfg(feature = "runtime-adapter")]
impl MultiBackendProvisioner {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<MeerkatMachine>>,
        workgraph_service: Option<meerkat::WorkGraphService>,
        external: Option<ExternalBackendConfig>,
        supervisor_bridge: Arc<super::MobSupervisorBridge>,
    ) -> Self {
        let session =
            SessionBackend::new(session_service.clone(), runtime_adapter, workgraph_service);
        Self {
            session,
            external,
            supervisor_bridge,
            binding_persistence: None,
        }
    }

    pub(super) async fn retire_exact_attachment_for_explicit_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, MobError> {
        self.session
            .retire_exact_attachment_for_explicit_resume(session_id)
            .await
    }

    pub fn with_binding_persistence(
        mut self,
        mob_id: crate::MobId,
        runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    ) -> Self {
        self.binding_persistence = Some(ProvisionerBindingPersistence {
            mob_id,
            runtime_metadata,
        });
        self
    }

    async fn peer_only_spec(
        &self,
        member_ref: &MemberRef,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                session_id: None,
                ..
            } => Self::peer_only_spec_from_parts(peer_id, address, *pubkey),
            _ => Err(MobError::Internal(
                "peer-only spec requested for non-peer-only member".to_string(),
            )),
        }
    }

    fn peer_only_spec_from_parts(
        peer_id: &str,
        address: &str,
        pubkey: [u8; 32],
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let peer_name = address
            .strip_prefix("inproc://")
            .map(|value| value.split('?').next().unwrap_or(value).to_string())
            .unwrap_or_else(|| format!("mob_member/backend_peer/{peer_id}"));
        let result = TrustedPeerDescriptor::unsigned_with_pubkey(
            peer_name,
            peer_id.to_string(),
            pubkey,
            address.to_string(),
        );
        result.map_err(|error| MobError::WiringError(format!("invalid peer-only spec: {error}")))
    }

    fn validated_external_peer_spec(
        peer_name: &str,
        peer_id: &str,
        address: &str,
        pubkey: [u8; 32],
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let result = TrustedPeerDescriptor::unsigned_with_pubkey(
            peer_name.to_string(),
            peer_id.to_string(),
            pubkey,
            address.to_string(),
        );
        result.map_err(|error| {
            MobError::WiringError(format!(
                "invalid external peer spec for '{peer_name}': {error}"
            ))
        })
    }

    fn bridge_bootstrap_token_from_binding(
        address: &str,
        bootstrap_token: Option<&str>,
    ) -> Result<super::bridge_protocol::BridgeBootstrapToken, MobError> {
        bootstrap_token
            .filter(|token| !token.is_empty())
            .map(super::bridge_protocol::BridgeBootstrapToken::new)
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "external runtime binding for '{address}' is missing typed bootstrap_token field"
                ))
            })
    }

    async fn bridge_supervisor_payload(
        &self,
    ) -> Result<super::bridge_protocol::BridgeSupervisorPayload, MobError> {
        let authority = self.supervisor_bridge.authority().await;
        let spec = self.supervisor_bridge.supervisor_spec().await?;
        Ok(super::bridge_protocol::BridgeSupervisorPayload {
            supervisor: spec.into(),
            epoch: authority.epoch,
            protocol_version: authority.protocol_version,
        })
    }

    async fn bridge_supervisor_payload_for_recipient(
        &self,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<super::bridge_protocol::BridgeSupervisorPayload, MobError> {
        let authority = self.supervisor_bridge.authority().await;
        let spec = self
            .supervisor_bridge
            .supervisor_spec_for_recipient(recipient)
            .await?;
        Ok(super::bridge_protocol::BridgeSupervisorPayload {
            supervisor: spec.into(),
            epoch: authority.epoch,
            protocol_version: authority.protocol_version,
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

    /// `trust_recipient` mutates the supervisor bridge DSL before reconciling
    /// the live comms row.  Its error therefore cannot certify that trust is
    /// absent.  The provisioner cannot close the actor-owned
    /// `pending_recipient_trust` obligation, so surface the existing typed
    /// cleanup-uncertain class and let the machine owner retain/fail-stop it.
    fn uncertain_recipient_trust_install(error: MobError) -> MobError {
        MobError::ExternalMemberCleanupUncertain {
            reason: format!(
                "recipient trust install failed after its declarative projection may have committed: {error}; trust state and the machine-owned pending obligation must be retained for cold repair"
            ),
        }
    }

    async fn ensure_supervisor_authorized(
        &self,
        peer: &TrustedPeerDescriptor,
        binding: Option<PeerOnlyBindingParts<'_>>,
        rebind_authority: Option<&PeerOnlyRebindAuthority>,
    ) -> Result<PeerOnlySupervisorAuthorization, MobError> {
        if let Some((current, pending)) = self.pending_supervisor_rotation().await?
            && pending
                .accepted_peer_ids
                .iter()
                .any(|peer_id| peer_id == &peer.peer_id.to_string())
        {
            return Err(Self::pending_supervisor_rotation_blocks_rebind(
                &current, &pending, peer,
            ));
        }
        let payload = self.bridge_supervisor_payload_for_recipient(peer).await?;
        let protocol_version = payload.protocol_version;
        let command = super::bridge_protocol::BridgeCommand::AuthorizeSupervisor(payload);
        // Transport requires the recipient be trusted before the request can be
        // routed (see comms admission), so trust is installed before the send.
        // The invariant is enforced by rolling trust newly installed by THIS
        // attempt back on every path where this `peer` is not a CONFIRMED,
        // ACCEPTED supervisor (fail closed). Trust that pre-existed the call
        // was established by an earlier confirmed terminality and is left in
        // place. The corresponding MobMachine `pending_recipient_trust`
        // obligation is owned by the actor around external provisioning. The
        // provisioner can read the durable pending rotation to refuse an old-
        // authority rebind, but cannot rewrite that MobMachine-owned fact.
        let install = self
            .supervisor_bridge
            .trust_recipient(peer)
            .await
            .map_err(Self::uncertain_recipient_trust_install)?;
        // 60s (not 30s): the requester must tolerate the same async
        // trust/peer-registration propagation lag the live peer waits out
        // before replying (see `spawn_live_external_peer`, 60s). It bounds only
        // genuine failure-lag — the happy path returns as soon as the reply
        // lands. Under high-parallelism RBE, loopback registration can lag tens
        // of seconds; in real deployments it propagates in milliseconds.
        let value = match self
            .supervisor_bridge
            .send_bridge_command(peer, &command, Duration::from_secs(60))
            .await
        {
            Ok(value) => value,
            Err(send_error) => {
                return Err(self
                    .rollback_supervisor_recipient_trust(peer, install, send_error)
                    .await);
            }
        };
        if let Some(rejection) = Self::bridge_rejection_reply(protocol_version, &value) {
            // The provisioner does NOT own the recoverable-vs-fatal verdict for
            // a rejection cause — that is a MobMachine-owned fact. When a
            // binding is present we hoist the raw typed cause up to the
            // machine-owning caller (resume builder) instead of self-classifying.
            // When `rebind_authority` is already present, the caller has classified
            // the cause as recoverable and minted the rebind authority, so we
            // proceed with the inline bind path.
            if let Some(cause) = rejection.typed_cause()
                && let Some((peer_id, address, bootstrap_token, pubkey)) = binding
            {
                let effective_bootstrap_token =
                    match Self::bridge_bootstrap_token_from_binding(address, bootstrap_token) {
                        Ok(token) => token,
                        Err(binding_error) => {
                            return Err(self
                                .rollback_supervisor_recipient_trust(peer, install, binding_error)
                                .await);
                        }
                    };
                let expected_address = super::bridge_protocol::canonicalize_bridge_address(address);
                let expected_peer =
                    match Self::peer_only_spec_from_parts(peer_id, &expected_address, pubkey) {
                        Ok(peer) => peer,
                        Err(peer_error) => {
                            return Err(self
                                .rollback_supervisor_recipient_trust(peer, install, peer_error)
                                .await);
                        }
                    };
                let Some(rebind_authority) = rebind_authority else {
                    // No rebind authority yet: hoist the observation so the
                    // machine-owning caller can classify and re-attempt. Trust
                    // newly installed for this rejected attempt must not
                    // survive between attempts; pre-existing confirmed trust
                    // is left in place.
                    if install == super::supervisor_bridge::RecipientTrustInstall::NewlyInstalled
                        && let Err(untrust_error) =
                            self.supervisor_bridge.untrust_recipient(peer).await
                    {
                        return Err(MobError::ExternalMemberCleanupUncertain {
                            reason: format!(
                                "AuthorizeSupervisor rejection for '{peer_id}' could not roll back rejected recipient trust: {untrust_error}; trust and the machine-owned pending obligation must be retained"
                            ),
                        });
                    }
                    return Ok(PeerOnlySupervisorAuthorization {
                        peer: expected_peer.clone(),
                        rebind_required: Some(PeerOnlyRebindObservation {
                            observed_peer: expected_peer,
                            bootstrap_token: effective_bootstrap_token,
                            rejection_cause: cause,
                        }),
                    });
                };
                let pending_rotation = match self.pending_supervisor_rotation().await {
                    Ok(pending_rotation) => pending_rotation,
                    Err(load_error) => {
                        return Err(self
                            .rollback_supervisor_recipient_trust(peer, install, load_error)
                            .await);
                    }
                };
                if let Some((current, pending)) = pending_rotation {
                    let pending_error =
                        Self::pending_supervisor_rotation_blocks_rebind(&current, &pending, peer);
                    return Err(self
                        .rollback_supervisor_recipient_trust(peer, install, pending_error)
                        .await);
                }
                if rebind_authority.peer.name != expected_peer.name
                    || rebind_authority.peer.peer_id != expected_peer.peer_id
                    || rebind_authority.peer.address != expected_peer.address
                    || rebind_authority.peer.pubkey != expected_peer.pubkey
                {
                    // The rejected recipient cannot be rebound to a matching
                    // authority, so it is not a confirmed supervisor and its
                    // trust must not survive.
                    let mismatch = MobError::WiringError(format!(
                        "peer-only rebind authority for '{peer_id}' does not match MobMachine member peer endpoint"
                    ));
                    return Err(self
                        .rollback_supervisor_recipient_trust(peer, install, mismatch)
                        .await);
                }
                // Re-bind the same peer (the bind observes/reuses the outer
                // recipient trust). A certified pre-send failure or rejection
                // rolls that trust back; sent-but-unterminated ambiguity keeps
                // it as exact reconciliation authority.
                let bind: super::bridge_protocol::BridgeBindResponse = match self
                    .bind_peer_only_member(peer, peer_id, address, bootstrap_token)
                    .await
                {
                    Ok((bind, _bind_trust_install)) => bind,
                    Err(bind_error) if bind_error.external_member_cleanup_is_uncertain() => {
                        // The inner BindMember reused the outer trust edge
                        // (`AlreadyTrusted`) and crossed its send boundary.
                        // Rolling back the outer `NewlyInstalled` token here
                        // would remove the very trust retained for exact
                        // rebind reconciliation.
                        return Err(bind_error);
                    }
                    Err(bind_error) => {
                        return Err(self
                            .rollback_supervisor_recipient_trust(peer, install, bind_error)
                            .await);
                    }
                };
                let returned_address =
                    super::bridge_protocol::canonicalize_bridge_address(&bind.address);
                if bind.peer_id != peer_id || returned_address != expected_address {
                    return Err(MobError::ExternalMemberCleanupUncertain {
                        reason: format!(
                            "peer-only rebind committed remotely but returned a different endpoint for '{peer_id}'; recipient trust retained for machine-authorized exact reconciliation"
                        ),
                    });
                }
                return Ok(PeerOnlySupervisorAuthorization {
                    peer: expected_peer,
                    rebind_required: None,
                });
            }
            return Err(self
                .rollback_supervisor_recipient_trust(
                    peer,
                    install,
                    Self::bridge_rejection_error(rejection),
                )
                .await);
        }
        let _ack = match super::bridge_protocol::decode_bridge_ack(
            &command,
            value,
            "authorize supervisor response",
        ) {
            Ok(ack) => ack,
            Err(decode_error) => {
                return Err(self
                    .rollback_supervisor_recipient_trust(peer, install, decode_error)
                    .await);
            }
        };
        Ok(PeerOnlySupervisorAuthorization {
            peer: peer.clone(),
            rebind_required: None,
        })
    }

    /// Fail-closed rollback for supervisor recipient trust installed ahead of an
    /// `AuthorizeSupervisor` send. The recipient was trusted only so the bridge
    /// request could be routed; if authorization did not terminate in a
    /// confirmed accept, trust newly installed by this attempt must not
    /// survive. Trust that pre-existed the attempt was established by an
    /// earlier confirmed terminality and is left in place. The authorization
    /// `original_error` is preserved; an untrust failure is folded into a typed
    /// `MobError` rather than silently dropped.
    async fn rollback_supervisor_recipient_trust(
        &self,
        peer: &TrustedPeerDescriptor,
        install: super::supervisor_bridge::RecipientTrustInstall,
        original_error: MobError,
    ) -> MobError {
        if install == super::supervisor_bridge::RecipientTrustInstall::NewlyInstalled
            && let Err(untrust_error) = self.supervisor_bridge.untrust_recipient(peer).await
        {
            return MobError::ExternalMemberCleanupUncertain {
                reason: format!(
                    "supervisor authorization failed ({original_error}); additionally failed to roll back installed recipient trust: {untrust_error}"
                ),
            };
        }
        original_error
    }

    async fn pending_supervisor_rotation(
        &self,
    ) -> Result<
        Option<(
            crate::store::SupervisorAuthorityRecord,
            crate::store::SupervisorPendingRotationRecord,
        )>,
        MobError,
    > {
        let Some(persistence) = self.binding_persistence.as_ref() else {
            return Ok(None);
        };
        let Some(current) = persistence
            .runtime_metadata
            .load_supervisor_authority(&persistence.mob_id)
            .await?
        else {
            return Ok(None);
        };
        let Some(pending) = current.pending_rotation.clone() else {
            return Ok(None);
        };
        Ok(Some((current, pending)))
    }

    fn pending_supervisor_rotation_blocks_rebind(
        current: &crate::store::SupervisorAuthorityRecord,
        pending: &crate::store::SupervisorPendingRotationRecord,
        peer: &TrustedPeerDescriptor,
    ) -> MobError {
        let operation = pending.operation_id.map_or_else(
            || "legacy operation awaiting migration".to_string(),
            |operation_id| format!("operation {operation_id}"),
        );
        MobError::SupervisorRotationIncomplete {
            previous_epoch: current.epoch,
            attempted_epoch: pending.epoch,
            attempted_public_peer_id: pending.public_peer_id.clone(),
            rotated_peer_count: pending.accepted_peer_ids.len(),
            rollback_succeeded: false,
            pending_authority_recorded: true,
            rollback_error: None,
            reason: format!(
                "supervisor rotation {operation} remains pending; refusing to reauthorize old authority for peer '{}'",
                peer.peer_id
            ),
        }
    }

    async fn send_bridge_command_typed<R: super::bridge_protocol::FromBridgeReply>(
        &self,
        peer: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<R, MobError> {
        self.send_bridge_command_typed_with_trust_install(peer, command, timeout)
            .await
            .map(|(response, _)| response)
    }

    async fn send_bridge_command_typed_with_trust_install<
        R: super::bridge_protocol::FromBridgeReply,
    >(
        &self,
        peer: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<(R, super::supervisor_bridge::RecipientTrustInstall), MobError> {
        // Trust is installed ahead of the routed send (comms admission).
        // Every non-confirmed outcome — send failure, terminal rejection,
        // decode failure — rolls trust newly installed by THIS call back
        // fail-closed, mirroring the actor's `ensure_supervisor_authorized`;
        // pre-existing confirmed trust is left in place. The MobMachine
        // `pending_recipient_trust` obligation for this window is owned by
        // the actor around external provisioning (the provisioner cannot
        // reach MobMachine authority).
        let install = self
            .supervisor_bridge
            .trust_recipient(peer)
            .await
            .map_err(Self::uncertain_recipient_trust_install)?;
        let value = match self
            .supervisor_bridge
            .send_bridge_command(peer, command, timeout)
            .await
        {
            Ok(value) => value,
            Err(send_error) => {
                return Err(self
                    .rollback_supervisor_recipient_trust(peer, install, send_error)
                    .await);
            }
        };
        if let Some(rejection) = Self::bridge_rejection_reply(command.protocol_version(), &value) {
            return Err(self
                .rollback_supervisor_recipient_trust(
                    peer,
                    install,
                    Self::bridge_rejection_error(rejection),
                )
                .await);
        }
        match super::bridge_protocol::decode_bridge_payload(command, value, "command") {
            Ok(payload) => Ok((payload, install)),
            Err(decode_error) => Err(self
                .rollback_supervisor_recipient_trust(peer, install, decode_error)
                .await),
        }
    }

    /// [`Self::send_bridge_command_typed`] variant surfacing the request
    /// envelope id (DEC-P6E-19) with the same trust install/rollback
    /// discipline.
    async fn send_bridge_command_typed_with_receipt<R: super::bridge_protocol::FromBridgeReply>(
        &self,
        peer: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<(R, uuid::Uuid), MobError> {
        let install = self
            .supervisor_bridge
            .trust_recipient(peer)
            .await
            .map_err(Self::uncertain_recipient_trust_install)?;
        let (value, envelope_id) = match self
            .supervisor_bridge
            .send_bridge_command_with_receipt(peer, command, timeout)
            .await
        {
            Ok(reply) => reply,
            Err(send_error) => {
                return Err(self
                    .rollback_supervisor_recipient_trust(peer, install, send_error)
                    .await);
            }
        };
        if let Some(rejection) = Self::bridge_rejection_reply(command.protocol_version(), &value) {
            return Err(self
                .rollback_supervisor_recipient_trust(
                    peer,
                    install,
                    Self::bridge_rejection_error(rejection),
                )
                .await);
        }
        match super::bridge_protocol::decode_bridge_payload(command, value, "command") {
            Ok(payload) => Ok((payload, envelope_id)),
            Err(decode_error) => Err(self
                .rollback_supervisor_recipient_trust(peer, install, decode_error)
                .await),
        }
    }

    async fn bind_peer_only_member(
        &self,
        peer: &TrustedPeerDescriptor,
        peer_id: &str,
        address: &str,
        bootstrap_token: Option<&str>,
    ) -> Result<
        (
            super::bridge_protocol::BridgeBindResponse,
            super::supervisor_bridge::RecipientTrustInstall,
        ),
        MobError,
    > {
        let bootstrap_token = Self::bridge_bootstrap_token_from_binding(address, bootstrap_token)?;
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self
            .supervisor_bridge
            .supervisor_spec_for_recipient(peer)
            .await?;
        let command = super::bridge_protocol::BridgeCommand::BindMember(
            super::bridge_protocol::BridgeBindPayload {
                supervisor: sup_spec.into(),
                epoch: authority.epoch,
                protocol_version: authority.protocol_version,
                expected_peer_id: peer_id.to_string(),
                expected_address: address.to_string(),
                bootstrap_token,
            },
        );
        // 60s (not 30s): tolerate async trust/peer-registration propagation lag
        // before the bind reply lands (matches the responder's 60s wait). Bounds
        // failure-lag only; under high-parallelism RBE loopback registration can
        // lag tens of seconds, in real deployments it propagates in ms.
        let install = self
            .supervisor_bridge
            .trust_recipient(peer)
            .await
            .map_err(Self::uncertain_recipient_trust_install)?;
        let value = match self
            .supervisor_bridge
            .send_bridge_command_classified(peer, &command, Duration::from_secs(60))
            .await
        {
            Ok(value) => value,
            Err(super::supervisor_bridge::BridgeRequestFailure::BeforeSend(error)) => {
                return Err(self
                    .rollback_supervisor_recipient_trust(peer, install, error)
                    .await);
            }
            Err(super::supervisor_bridge::BridgeRequestFailure::AfterSend(error)) => {
                // The receiver may have committed BindMember before its
                // terminal response was lost. Keep both recipient trust and
                // the caller's exact PeerId reservation available for cold
                // cleanup; rolling either back would falsely certify absence.
                return Err(MobError::ExternalMemberCleanupUncertain {
                    reason: format!(
                        "BindMember request was sent but no authenticated terminal response confirmed its outcome: {error}; recipient trust retained"
                    ),
                });
            }
        };
        if let Some(rejection) = Self::bridge_rejection_reply(command.protocol_version(), &value) {
            // An authenticated command rejection is the only post-send reply
            // that certifies BindMember made no mutation.
            return Err(self
                .rollback_supervisor_recipient_trust(
                    peer,
                    install,
                    Self::bridge_rejection_error(rejection),
                )
                .await);
        }
        match super::bridge_protocol::decode_bridge_payload(&command, value, "BindMember command") {
            Ok(payload) => Ok((payload, install)),
            Err(error) => Err(MobError::ExternalMemberCleanupUncertain {
                reason: format!(
                    "BindMember returned an unauthenticated or undecodable terminal response after send: {error}; recipient trust retained"
                ),
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn compensate_failed_external_member_after_bind(
        &self,
        peer: &TrustedPeerDescriptor,
        member_ref: &MemberRef,
        owner_bridge_session_id: &SessionId,
        peer_name: &str,
        operation_source: &OperationSource,
        operation_id: &OperationId,
        trust_install: super::supervisor_bridge::RecipientTrustInstall,
        original_error: MobError,
    ) -> MobError {
        let retire_result = match self.bridge_supervisor_payload_for_recipient(peer).await {
            Ok(payload) => {
                let command = super::bridge_protocol::BridgeCommand::RetireMember(payload);
                self.send_bridge_command_typed::<super::bridge_protocol::BridgeRetireResponse>(
                    peer,
                    &command,
                    Duration::from_secs(10),
                )
                .await
                .map(|_| ())
            }
            Err(error) => Err(error),
        };
        if let Err(error) = retire_result {
            // BindMember may have committed remotely. The exact operation and
            // PeerId binding are the quarantine authority until RetireMember
            // is confirmed; trust stays installed so cleanup can be retried.
            return MobError::ExternalMemberCleanupUncertain {
                reason: format!(
                    "external member provisioning failed after BindMember ({original_error}); compensating RetireMember was not confirmed: {error}; exact operation '{operation_id}' and PeerId reservation retained"
                ),
            };
        }

        // Cleanup ordering is deliberate: a residual trusted peer must not be
        // allowed to reuse this PeerId after the exact reservation is cleared.
        // Therefore confirmed remote retirement is followed by confirmed
        // rollback of trust newly installed by this attempt, and only then by
        // exact local operation terminalization/binding removal.
        if trust_install == super::supervisor_bridge::RecipientTrustInstall::NewlyInstalled
            && let Err(error) = self.supervisor_bridge.untrust_recipient(peer).await
        {
            return MobError::ExternalMemberCleanupUncertain {
                reason: format!(
                    "external member provisioning failed after BindMember ({original_error}); remote RetireMember confirmed but recipient trust cleanup failed: {error}; exact operation '{operation_id}' and PeerId reservation retained"
                ),
            };
        }

        if let Err(error) = self
            .session
            .ops_adapter
            .abort_generic_member_provision_exact(
                member_ref,
                owner_bridge_session_id,
                peer_name,
                operation_source,
                operation_id,
                "external provisioning failed after BindMember",
            )
        {
            return MobError::ExternalMemberCleanupUncertain {
                reason: format!(
                    "external member provisioning failed after BindMember ({original_error}); remote retirement and recipient trust cleanup confirmed but exact operation cleanup failed: {error}; PeerId reservation retained"
                ),
            };
        }
        original_error
    }

    async fn external_member_ref(
        &self,
        _create_session: CreateSessionRequest,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
        target: ExternalBindingTarget,
    ) -> Result<MemberSpawnReceipt, MobError> {
        let ExternalBindingTarget {
            peer_name,
            peer_id: real_peer_id,
            address: real_address,
            bootstrap_token,
            pubkey,
        } = target;
        let effective_bootstrap_token = Self::bridge_bootstrap_token_from_binding(
            &real_address,
            bootstrap_token
                .as_ref()
                .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
        )?;
        if peer_name.parse::<meerkat_core::MemberCommsName>().is_err() {
            return Err(MobError::WiringError(format!(
                "invalid external peer name '{peer_name}': expected '<mob>/<profile>/<meerkat>' using identifier-safe segments"
            )));
        }
        tracing::debug!(
            peer_name = %peer_name,
            "MultiBackendProvisioner::external_member_ref start"
        );
        let _external = self
            .external
            .as_ref()
            .ok_or_else(|| MobError::WiringError("external backend is not configured".into()))?;
        tracing::debug!(
            peer_id = %real_peer_id,
            address = %real_address,
            "MultiBackendProvisioner::external_member_ref success (real identity)"
        );
        let canonical_requested_address =
            super::bridge_protocol::canonicalize_bridge_address(&real_address);
        let peer = Self::validated_external_peer_spec(
            &peer_name,
            &real_peer_id,
            &canonical_requested_address,
            pubkey,
        )?;
        let operation_source = OperationSource::backend_peer(peer.peer_id, peer.address.clone());
        let bind_bootstrap_token = effective_bootstrap_token.clone();
        let member_ref = MemberRef::BackendPeer {
            peer_id: peer.peer_id.to_string(),
            address: peer.address.to_string(),
            pubkey,
            bootstrap_token: Some(effective_bootstrap_token),
            session_id: None,
        };
        // Reserve the canonical PeerId/endpoint in the local operation adapter
        // before BindMember installs trust or remote supervisor authority.
        // Collision denial is therefore certified no-side-effect.
        let operation_id = self.session.ops_adapter.reserve_generic_member_provision(
            &member_ref,
            owner_bridge_session_id.clone(),
            Arc::clone(&ops_registry),
            peer_name.clone(),
            operation_source.clone(),
        )?;
        let (bind_response, trust_install) = match self
            .bind_peer_only_member(
                &peer,
                &real_peer_id,
                &canonical_requested_address,
                Some(bind_bootstrap_token.as_str()),
            )
            .await
        {
            Ok(response) => response,
            Err(error) if error.external_member_cleanup_is_uncertain() => {
                // The BindMember request crossed the send boundary (or its
                // trust rollback itself became uncertain). The exact generic
                // operation is the durable PeerId quarantine authority and
                // must survive for cold cleanup/reconciliation.
                return Err(error);
            }
            Err(error) => {
                if let Err(cleanup_error) = self
                    .session
                    .ops_adapter
                    .abort_generic_member_provision_exact(
                        &member_ref,
                        &owner_bridge_session_id,
                        &peer_name,
                        &operation_source,
                        &operation_id,
                        "BindMember was not confirmed",
                    )
                {
                    let reason = format!(
                        "external BindMember failed ({error}); additionally failed to abort exact pre-side-effect PeerId reservation: {cleanup_error}"
                    );
                    return Err(MobError::ExternalMemberCleanupUncertain { reason });
                }
                return Err(error);
            }
        };
        let canonical_address =
            super::bridge_protocol::canonicalize_bridge_address(&bind_response.address);
        let validated_bind_response = match Self::validated_external_peer_spec(
            &peer_name,
            &bind_response.peer_id,
            &canonical_address,
            pubkey,
        ) {
            Ok(validated) => validated,
            Err(error) => {
                return Err(self
                    .compensate_failed_external_member_after_bind(
                        &peer,
                        &member_ref,
                        &owner_bridge_session_id,
                        &peer_name,
                        &operation_source,
                        &operation_id,
                        trust_install,
                        error,
                    )
                    .await);
            }
        };
        if validated_bind_response.peer_id != peer.peer_id
            || validated_bind_response.address != peer.address
        {
            let error = MobError::WiringError(format!(
                "external BindMember response for '{peer_name}' changed the reserved PeerId/address tuple"
            ));
            return Err(self
                .compensate_failed_external_member_after_bind(
                    &peer,
                    &member_ref,
                    &owner_bridge_session_id,
                    &peer_name,
                    &operation_source,
                    &operation_id,
                    trust_install,
                    error,
                )
                .await);
        }
        if let Err(error) = self
            .session
            .ops_adapter
            .complete_generic_member_provision_exact(
                &member_ref,
                &owner_bridge_session_id,
                &peer_name,
                &operation_source,
                &operation_id,
            )
        {
            return Err(self
                .compensate_failed_external_member_after_bind(
                    &peer,
                    &member_ref,
                    &owner_bridge_session_id,
                    &peer_name,
                    &operation_source,
                    &operation_id,
                    trust_install,
                    error,
                )
                .await);
        }
        Ok(MemberSpawnReceipt {
            member_ref,
            operation_id,
            session_origin: ProvisionSessionOrigin::Fresh,
            rollback_authority: None,
            materialized_ack: None,
            failed_restore_peer_ids: Vec::new(),
        })
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobProvisioner for MultiBackendProvisioner {
    fn supports_member_turn_llm_reconfigure(&self, member_ref: &MemberRef) -> bool {
        match member_ref {
            MemberRef::Session { .. } => self
                .session
                .supports_member_turn_llm_reconfigure(member_ref),
            MemberRef::BackendPeer { .. } => false,
        }
    }

    async fn provision_member(
        &self,
        mut req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError> {
        match req.binding {
            RuntimeBinding::Session => {
                self.session
                    .provision_member(ProvisionMemberRequest {
                        create_session: req.create_session,
                        session_origin: req.session_origin,
                        binding: RuntimeBinding::Session,
                        peer_name: req.peer_name,
                        owner_bridge_session_id: req.owner_bridge_session_id,
                        ops_registry: req.ops_registry,
                        generated_self_owned_operation_owner: req
                            .generated_self_owned_operation_owner,
                        runtime_revival_intent: req.runtime_revival_intent,
                    })
                    .await
            }
            RuntimeBinding::External {
                peer_id,
                address,
                bootstrap_token,
                pubkey,
            } => {
                let (owner_bridge_session_id, ops_registry) =
                    match (req.owner_bridge_session_id.take(), req.ops_registry.take()) {
                        (Some(owner_bridge_session_id), Some(ops_registry)) => {
                            (owner_bridge_session_id, ops_registry)
                        }
                        (None, None) => {
                            return Err(MobError::Internal(
                            "external peer-only member operation requires generated owner binding"
                                .into(),
                        ));
                        }
                        _ => {
                            return Err(MobError::Internal(
                                "mob provisioner received partial ops owner binding".into(),
                            ));
                        }
                    };
                self.external_member_ref(
                    req.create_session,
                    owner_bridge_session_id,
                    ops_registry,
                    ExternalBindingTarget {
                        peer_name: req.peer_name,
                        peer_id,
                        address,
                        bootstrap_token,
                        pubkey,
                    },
                )
                .await
            }
            RuntimeBinding::HostMaterialized { host } => Err(MobError::Internal(format!(
                "host-materialized members are materialized via the bridge (host '{}'), never provisioned locally",
                host.as_str()
            ))),
        }
    }

    async fn materialize_member(
        &self,
        req: MaterializeMemberRequest,
    ) -> Result<MaterializedSpawnReceipt, MobError> {
        let MaterializeMemberRequest {
            host_peer,
            payload,
            peer_name,
            owner_bridge_session_id,
            ops_registry,
            provision_operation_id,
            operation_anchor,
            timeout,
        } = req;
        if peer_name.parse::<meerkat_core::MemberCommsName>().is_err() {
            return Err(MobError::WiringError(format!(
                "invalid member peer name '{peer_name}': expected '<mob>/<profile>/<meerkat>' using identifier-safe segments"
            )));
        }
        let host_peer_id = host_peer.peer_id.to_string();
        let supervisor_peer_id = payload.supervisor.peer_id.clone();
        let command = super::bridge_protocol::BridgeCommand::MaterializeMember(payload);
        // ADJ-4: exactly ONE resend of the byte-identical command at the
        // same idempotency tuple, only on send-timeout / wire `Unavailable`
        // (host-side Replay admission makes the resend safe by
        // construction). Every other cause is terminal.
        let ack: super::bridge_protocol::BridgeMaterializedResponse = match self
            .send_bridge_command_typed(&host_peer, &command, timeout)
            .await
        {
            Ok(ack) => ack,
            Err(error) if idempotent_bridge_resend_class(&error) => {
                tracing::warn!(
                    peer_name = %peer_name,
                    error = %error,
                    "MaterializeMember send failed transient; resending once at the same tuple"
                );
                self.send_bridge_command_typed(&host_peer, &command, timeout)
                    .await?
            }
            Err(error) => return Err(error),
        };

        // Transport-integrity validation (fail closed, before any ops
        // write): canonical address, pubkey decode + peer-id derivation,
        // non-empty session id. Digest/engine echoes are machine guards.
        validate_materialized_member_principal_disjointness(
            &peer_name,
            &ack.member_peer_id,
            &host_peer_id,
            &supervisor_peer_id,
        )?;
        let canonical_address =
            super::bridge_protocol::canonicalize_bridge_address(&ack.advertised_address);
        let member_pubkey = decode_ack_member_pubkey(&ack.member_pubkey)?;
        let member_peer = TrustedPeerDescriptor::unsigned_with_pubkey(
            peer_name.clone(),
            ack.member_peer_id.clone(),
            member_pubkey,
            canonical_address.clone(),
        )
        .map_err(|error| {
            MobError::WiringError(format!(
                "materialize ack for '{peer_name}' does not form a canonical member peer descriptor: {error}"
            ))
        })?;
        let session_id = SessionId::parse(&ack.session_id).map_err(|error| {
            MobError::WiringError(format!(
                "materialize ack for '{peer_name}' carries an invalid session id '{}': {error}",
                ack.session_id
            ))
        })?;

        // DEC-P3-6: the shell ref carries PEER facts only; the remote
        // session binding is machine truth (`member_session_bindings` via
        // the remote commit) — session_id: None keeps every local-session
        // inference honest.
        let member_ref = MemberRef::BackendPeer {
            peer_id: ack.member_peer_id.clone(),
            address: canonical_address,
            pubkey: member_pubkey,
            bootstrap_token: None,
            session_id: None,
        };
        let operation_source =
            OperationSource::backend_peer(member_peer.peer_id, member_peer.address.clone());
        self.session
            .ops_adapter
            .bind_member_registry_for_exact_operation(
                &member_ref,
                owner_bridge_session_id.clone(),
                ops_registry,
                peer_name.clone(),
                operation_source,
                provision_operation_id.clone(),
            )?;
        let operation_id = match operation_anchor {
            PlacedProvisionOperationAnchor::NewPending => {
                self.session
                    .ops_adapter
                    .mark_member_provisioned_for_member_exact(
                        &member_ref,
                        &peer_name,
                        &provision_operation_id,
                    )
                    .await?
            }
            PlacedProvisionOperationAnchor::ExistingCommitted => {
                self.session
                    .ops_adapter
                    .verify_running_placed_member_operation_exact(
                        &member_ref,
                        &owner_bridge_session_id,
                        &peer_name,
                        &provision_operation_id,
                    )
                    .await?;
                provision_operation_id.clone()
            }
        };
        Ok(MaterializedSpawnReceipt {
            receipt: MemberSpawnReceipt {
                member_ref,
                operation_id,
                session_origin: ProvisionSessionOrigin::Fresh,
                rollback_authority: None,
                materialized_ack: Some(Box::new(MaterializedMemberAck {
                    member_peer,
                    session_id,
                    spec_digest_echo: ack.spec_digest.clone(),
                    engine_version: ack.engine_version.clone(),
                    launch_outcome: ack.launch_outcome,
                    resolved_auth_binding: ack.resolved_auth_binding.clone().map(|wire| {
                        meerkat_core::AuthBindingRef {
                            realm: wire.realm,
                            binding: wire.binding,
                            profile: wire.profile,
                            origin: Default::default(),
                        }
                    }),
                })),
                failed_restore_peer_ids: Vec::new(),
            },
        })
    }

    async fn release_host_member(
        &self,
        request: HostMemberReleaseRequest,
    ) -> Result<crate::machines::mob_machine::MemberSessionDisposal, MobError> {
        let HostMemberReleaseRequest {
            mob_id,
            agent_identity,
            generation,
            fence_token,
            supervisor_authority,
            supervisor,
            binding_generation,
            host,
        } = request;
        let command = super::bridge_protocol::BridgeCommand::ReleaseMember(
            super::bridge_protocol::BridgeReleasePayload {
                supervisor,
                epoch: supervisor_authority.epoch,
                binding_generation,
                // Keep the detached release on the exact immutable supervisor
                // authority snapshot used to sign and send it.
                protocol_version: supervisor_authority.protocol_version,
                mob_id: mob_id.to_string(),
                agent_identity,
                generation,
                fence_token,
            },
        );
        let value = self
            .supervisor_bridge
            .send_bridge_command_as_authority(
                &supervisor_authority,
                &host,
                &command,
                HOST_RELEASE_TIMEOUT,
            )
            .await?;
        if let Some(rejection) = Self::bridge_rejection_reply(command.protocol_version(), &value) {
            return Err(Self::bridge_rejection_error(rejection));
        }
        let released: super::bridge_protocol::BridgeMemberReleasedResponse =
            super::bridge_protocol::decode_bridge_payload(
                &command,
                value,
                "ReleaseMember command",
            )?;
        Ok(super::bridge_protocol::member_session_disposal_from_wire(
            &released.disposal,
        ))
    }

    async fn abort_placed_provision_operation(
        &self,
        operation_owner_session_id: &SessionId,
        provision_operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        let runtime_adapter = self.session.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(
                "placed provision operation cleanup requires MeerkatMachine runtime authority"
                    .to_string(),
            )
        })?;
        let bindings = runtime_adapter
            .prepare_local_session_bindings(operation_owner_session_id.clone())
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "failed to resolve placed operation-owner session '{operation_owner_session_id}': {error}"
                ))
            })?;
        if bindings.session_id() != operation_owner_session_id
            || !meerkat_runtime::session_runtime_bindings_have_machine_authority(&bindings)
        {
            return Err(MobError::Internal(format!(
                "placed operation-owner bindings did not match machine-authorized session '{operation_owner_session_id}'"
            )));
        }
        super::ops_adapter::MobOpsAdapter::abort_placed_provision_operation_exact(
            bindings.ops_lifecycle().as_ref(),
            operation_owner_session_id,
            provision_operation_id,
            display_name,
            Some("placed materialization did not commit".to_string()),
        )?;
        self.session.ops_adapter.clear_placed_member_binding_exact(
            operation_owner_session_id,
            provision_operation_id,
            display_name,
        )
    }

    async fn ensure_committed_placed_provision_operation(
        &self,
        operation_owner_session_id: &SessionId,
        provision_operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        let runtime_adapter = self.session.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(
                "committed placed operation validation requires MeerkatMachine runtime authority"
                    .to_string(),
            )
        })?;
        let bindings = runtime_adapter
            .prepare_local_session_bindings(operation_owner_session_id.clone())
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "failed to resolve committed placed operation-owner session '{operation_owner_session_id}': {error}"
                ))
            })?;
        if bindings.session_id() != operation_owner_session_id
            || !meerkat_runtime::session_runtime_bindings_have_machine_authority(&bindings)
        {
            return Err(MobError::Internal(format!(
                "committed placed operation-owner bindings did not match machine-authorized session '{operation_owner_session_id}'"
            )));
        }
        super::ops_adapter::MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            bindings.ops_lifecycle().as_ref(),
            operation_owner_session_id,
            provision_operation_id,
            display_name,
        )
    }

    async fn retire_committed_placed_provision_operation(
        &self,
        operation_owner_session_id: &SessionId,
        provision_operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        let runtime_adapter = self.session.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(
                "committed placed operation retirement requires MeerkatMachine runtime authority"
                    .to_string(),
            )
        })?;
        let bindings = runtime_adapter
            .prepare_local_session_bindings(operation_owner_session_id.clone())
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "failed to resolve retiring placed operation-owner session '{operation_owner_session_id}': {error}"
                ))
            })?;
        if bindings.session_id() != operation_owner_session_id
            || !meerkat_runtime::session_runtime_bindings_have_machine_authority(&bindings)
        {
            return Err(MobError::Internal(format!(
                "retiring placed operation-owner bindings did not match machine-authorized session '{operation_owner_session_id}'"
            )));
        }
        super::ops_adapter::MobOpsAdapter::retire_committed_placed_provision_operation_exact(
            bindings.ops_lifecycle().as_ref(),
            operation_owner_session_id,
            provision_operation_id,
            display_name,
        )?;
        self.session.ops_adapter.clear_placed_member_binding_exact(
            operation_owner_session_id,
            provision_operation_id,
            display_name,
        )
    }

    async fn abort_member_provision(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => {
                self.session
                    .ops_adapter
                    .abort_member_provision_for_member(
                        member_ref,
                        operation_id,
                        Some(reason.to_string()),
                    )
                    .await
            }
            _ => {
                self.session
                    .abort_member_provision(member_ref, operation_id, reason)
                    .await
            }
        }
    }

    async fn capture_resumed_member_rollback_authority(
        &self,
        member_ref: &MemberRef,
    ) -> Result<ResumedMemberRollbackAuthority, MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => Err(MobError::Internal(
                "peer-only provision cannot carry resumed session rollback authority".into(),
            )),
            _ => {
                self.session
                    .capture_resumed_member_rollback_authority(member_ref)
                    .await
            }
        }
    }

    async fn restore_resumed_member(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        original_origin: ProvisionSessionOrigin,
        rollback_authority: &ResumedMemberRollbackAuthority,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => Err(MobError::Internal(
                "peer-only provision cannot be classified as a resumed durable session".into(),
            )),
            _ => {
                self.session
                    .restore_resumed_member(
                        member_ref,
                        operation_id,
                        original_origin,
                        rollback_authority,
                    )
                    .await
            }
        }
    }

    async fn retire_member(
        &self,
        member_ref: &MemberRef,
    ) -> Result<crate::machines::mob_machine::MemberSessionDisposal, MobError> {
        tracing::debug!(
            member_ref = ?member_ref,
            "CompositeProvisioner::retire_member start"
        );
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                session_id: None,
                ..
            } => {
                let peer = self.peer_only_spec(member_ref).await?;
                let authorization = self
                    .ensure_supervisor_authorized(
                        &peer,
                        Some((
                            peer_id.as_str(),
                            address.as_str(),
                            bootstrap_token
                                .as_ref()
                                .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                            *pubkey,
                        )),
                        None,
                    )
                    .await?;
                if let Some(observation) = authorization.rebind_required {
                    // No MobMachine authority is reachable on the retire path,
                    // so any rejection is bubbled up uniformly. The recoverable-
                    // vs-fatal verdict is MobMachine-owned and not re-derived
                    // here; the raw rejection cause is surfaced as-is.
                    return Err(MobError::BridgeCommandRejected {
                        cause: observation.rejection_cause,
                        reason: "peer-only retire was rejected by the remote member".to_string(),
                    });
                }
                let peer = authorization.peer;
                let payload = self.bridge_supervisor_payload_for_recipient(&peer).await?;
                let command = super::bridge_protocol::BridgeCommand::RetireMember(payload);
                let _retire: super::bridge_protocol::BridgeRetireResponse = self
                    .send_bridge_command_typed(&peer, &command, Duration::from_secs(10))
                    .await?;
                self.session
                    .ops_adapter
                    .mark_member_retired(member_ref)
                    .await?;
                // The controlling side only asked the remote member's own host
                // to retire its runtime; the member's durable session (if any)
                // is owned by that host and no archive happened here.
                Ok(
                    crate::machines::mob_machine::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
                )
            }
            _ => self.session.retire_member(member_ref).await,
        }
    }

    async fn interrupt_member(
        &self,
        member_ref: &MemberRef,
        expected_member: Option<&super::bridge_protocol::BridgeMemberIncarnation>,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                session_id,
                ..
            } if expected_member.is_some() || session_id.is_none() => {
                // Placement authority, not the optional session copy on the
                // roster ref, selects the remote control lane. A materialized
                // member carries its host-resident session id for exact
                // fencing; submitting that id to the controller's local
                // session backend would target the wrong owner.
                let peer = Self::peer_only_spec_from_parts(peer_id, address, *pubkey)?;
                let authorization = self
                    .ensure_supervisor_authorized(
                        &peer,
                        Some((
                            peer_id.as_str(),
                            address.as_str(),
                            bootstrap_token
                                .as_ref()
                                .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                            *pubkey,
                        )),
                        None,
                    )
                    .await?;
                if let Some(observation) = authorization.rebind_required {
                    // No MobMachine authority is reachable on the interrupt
                    // path, so any rejection is bubbled up uniformly without
                    // re-deriving the MobMachine-owned recovery verdict.
                    return Err(MobError::BridgeCommandRejected {
                        cause: observation.rejection_cause,
                        reason: "peer-only interrupt was rejected by the remote member".to_string(),
                    });
                }
                let peer = authorization.peer;
                let supervisor = self.bridge_supervisor_payload_for_recipient(&peer).await?;
                let payload = super::bridge_protocol::BridgeInterruptPayload {
                    supervisor: supervisor.supervisor,
                    epoch: supervisor.epoch,
                    protocol_version: supervisor.protocol_version,
                    expected_member: expected_member.cloned(),
                };
                let command = super::bridge_protocol::BridgeCommand::InterruptMember(payload);
                let _ack: super::bridge_protocol::BridgeAck = self
                    .send_bridge_command_typed(&peer, &command, Duration::from_secs(5))
                    .await?;
                Ok(())
            }
            _ => {
                self.session
                    .interrupt_member(member_ref, expected_member)
                    .await
            }
        }
    }

    async fn hard_cancel_member(
        &self,
        member_ref: &MemberRef,
        reason: &str,
    ) -> Result<(), MobError> {
        // The UNPLACED lane: the local runtime-authority path. A legacy
        // peer-only external has no local session, so this stays a typed
        // reject (DEC-P6E-8's non-placed posture) — the bridge lane is the
        // separate placed verb below, selected by the actor's machine
        // placement fact (ADJ-24: never the ref shape).
        self.session.hard_cancel_member(member_ref, reason).await
    }

    async fn hard_cancel_placed_member(
        &self,
        member_ref: &MemberRef,
        expected_member: &super::bridge_protocol::BridgeMemberIncarnation,
        reason: &str,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                ..
            } => {
                // DEC-P6E-8: the placed branch sends the explicit
                // `HardCancelMember` bridge command (the `interrupt_member`
                // peer-branch shape). The controlling actor has already
                // gated on the recorded host `hard_cancel_member` capability
                // fact BEFORE dispatch (FLAG-P6E-8); the member drain's
                // machine-admitted arm is the receiver. The ref-carried
                // session id (if any) names the REMOTE resident session and
                // is irrelevant to the send.
                let peer = Self::peer_only_spec_from_parts(peer_id, address, *pubkey)?;
                let authorization = self
                    .ensure_supervisor_authorized(
                        &peer,
                        Some((
                            peer_id.as_str(),
                            address.as_str(),
                            bootstrap_token
                                .as_ref()
                                .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                            *pubkey,
                        )),
                        None,
                    )
                    .await?;
                if let Some(observation) = authorization.rebind_required {
                    return Err(MobError::BridgeCommandRejected {
                        cause: observation.rejection_cause,
                        reason: "hard cancel was rejected by the remote member".to_string(),
                    });
                }
                let peer = authorization.peer;
                let authority = self.supervisor_bridge.authority().await;
                let sup_spec = self
                    .supervisor_bridge
                    .supervisor_spec_for_recipient(&peer)
                    .await?;
                // Pin the current run BEFORE constructing the cancel. The
                // receiver compares this exact id under the runtime mutation
                // gate, so a run-A observation racing run-B startup can only
                // converge as "A already terminal" — it can never interrupt B.
                let observation_command = super::bridge_protocol::BridgeCommand::ObserveMember(
                    super::bridge_protocol::BridgeSupervisorPayload {
                        supervisor: sup_spec.clone().into(),
                        epoch: authority.epoch,
                        protocol_version: authority.protocol_version,
                    },
                );
                let observation: super::bridge_protocol::BridgeObservationResponse = self
                    .send_bridge_command_typed(
                        &peer,
                        &observation_command,
                        HARD_CANCEL_BRIDGE_TIMEOUT,
                    )
                    .await?;
                let expected_run_id =
                    observation
                        .current_run_id
                        .ok_or_else(|| MobError::BridgeCommandRejected {
                            cause: super::bridge_protocol::BridgeRejectionCause::Unavailable,
                            reason: "remote member has no current run to hard-cancel".to_string(),
                        })?;
                let expected_run_id = CoreRunId::from_uuid(
                    uuid::Uuid::parse_str(&expected_run_id).map_err(|error| {
                        MobError::Internal(format!(
                            "remote member returned invalid current_run_id '{expected_run_id}': {error}"
                        ))
                    })?,
                );
                let operation_id = OperationId::new();
                let command = super::bridge_protocol::BridgeCommand::HardCancelMember(
                    super::bridge_protocol::BridgeHardCancelPayload {
                        supervisor: sup_spec.into(),
                        epoch: authority.epoch,
                        protocol_version: super::bridge_protocol::BridgeProtocolVersion::V4,
                        expected_member: expected_member.clone(),
                        operation_id: operation_id.clone(),
                        expected_run_id: expected_run_id.clone(),
                        reason: reason.to_string(),
                    },
                );
                // Exactly one resend for reply loss / transient unavailability,
                // always byte-identical at the same operation_id and
                // expected_run_id. If run A ended and run B started after the
                // first reply was lost, the replay ACKs A's terminal truth and
                // leaves B untouched.
                let _ack: super::bridge_protocol::BridgeAck = match self
                    .send_bridge_command_typed(&peer, &command, HARD_CANCEL_BRIDGE_TIMEOUT)
                    .await
                {
                    Ok(ack) => ack,
                    Err(error) if idempotent_bridge_resend_class(&error) => {
                        tracing::warn!(
                            operation_id = %operation_id,
                            expected_run_id = %expected_run_id,
                            error = %error,
                            "HardCancelMember send failed transiently; resending the exact run-fenced operation once"
                        );
                        self.send_bridge_command_typed(&peer, &command, HARD_CANCEL_BRIDGE_TIMEOUT)
                            .await?
                    }
                    Err(error) => return Err(error),
                };
                Ok(())
            }
            _ => self.session.hard_cancel_member(member_ref, reason).await,
        }
    }

    async fn cancel_tracked_placed_input(
        &self,
        member_ref: &MemberRef,
        expected_member: &super::bridge_protocol::BridgeMemberIncarnation,
        input_id: &str,
    ) -> Result<super::bridge_protocol::BridgeTrackedInputCancelResponse, MobError> {
        let MemberRef::BackendPeer {
            peer_id,
            address,
            pubkey,
            bootstrap_token,
            session_id: route_session_id,
        } = member_ref
        else {
            return Err(MobError::Internal(
                "tracked placed-input cancellation requires a BackendPeer transport route"
                    .to_string(),
            ));
        };
        if !placed_route_session_matches_machine(
            route_session_id.as_ref(),
            &expected_member.member_session_id,
        ) {
            return Err(MobError::Internal(format!(
                "tracked placed-input cancellation session '{}' does not match its member route",
                expected_member.member_session_id
            )));
        }
        let parsed_input_id = uuid::Uuid::parse_str(input_id).map_err(|_| {
            MobError::Internal(
                "tracked placed-input cancellation input_id is not a UUID".to_string(),
            )
        })?;
        if parsed_input_id.is_nil() || parsed_input_id.to_string() != input_id {
            return Err(MobError::Internal(
                "tracked placed-input cancellation input_id must be a canonical non-nil UUID"
                    .to_string(),
            ));
        }
        let peer = Self::peer_only_spec_from_parts(peer_id, address, *pubkey)?;
        let authorization = self
            .ensure_supervisor_authorized(
                &peer,
                Some((
                    peer_id.as_str(),
                    address.as_str(),
                    bootstrap_token
                        .as_ref()
                        .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                    *pubkey,
                )),
                None,
            )
            .await?;
        if let Some(observation) = authorization.rebind_required {
            return Err(MobError::BridgeCommandRejected {
                cause: observation.rejection_cause,
                reason: "tracked input cancellation was rejected by the remote member".to_string(),
            });
        }
        let peer = authorization.peer;
        let authority = self.supervisor_bridge.authority().await;
        let supervisor = self
            .supervisor_bridge
            .supervisor_spec_for_recipient(&peer)
            .await?;
        let command = super::bridge_protocol::BridgeCommand::CancelTrackedMemberInput(
            super::bridge_protocol::BridgeTrackedInputCancelPayload {
                supervisor: supervisor.into(),
                epoch: authority.epoch,
                protocol_version: authority.protocol_version,
                expected_member: expected_member.clone(),
                input_id: input_id.to_string(),
            },
        );
        // Cancellation is level-triggered at the exact residency/key. A lost
        // first reply is ambiguous, so repeat the byte-identical command once;
        // never resend the work input and never mint a new cancellation key.
        let response: super::bridge_protocol::BridgeTrackedInputCancelResponse = match self
            .send_bridge_command_typed(&peer, &command, TRACKED_INPUT_CANCEL_BRIDGE_TIMEOUT)
            .await
        {
            Ok(response) => response,
            Err(error) if idempotent_bridge_resend_class(&error) => {
                self.send_bridge_command_typed(&peer, &command, TRACKED_INPUT_CANCEL_BRIDGE_TIMEOUT)
                    .await?
            }
            Err(error) => return Err(error),
        };
        validate_tracked_input_cancel_response(expected_member, input_id, &response)?;
        Ok(response)
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        self.start_turn_with_receipt(member_ref, req)
            .await
            .map(|_| ())
    }

    async fn start_turn_with_receipt(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<Option<String>, MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                session_id: None,
                ..
            } => {
                if req.event_tx.is_some() {
                    return Err(MobError::UnsupportedForMode {
                        mode: crate::MobRuntimeMode::TurnDriven,
                        reason: "tracked turn event streams are not supported for peer-only members in phase 1".to_string(),
                    });
                }
                let peer = self.peer_only_spec(member_ref).await?;
                let authorization = self
                    .ensure_supervisor_authorized(
                        &peer,
                        Some((
                            peer_id.as_str(),
                            address.as_str(),
                            bootstrap_token
                                .as_ref()
                                .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                            *pubkey,
                        )),
                        None,
                    )
                    .await?;
                if let Some(observation) = authorization.rebind_required {
                    // start_turn runs in a detached task with no MobMachine
                    // authority reachable, so any rejection is bubbled up
                    // uniformly without re-deriving the MobMachine-owned
                    // recovery verdict.
                    return Err(MobError::BridgeCommandRejected {
                        cause: observation.rejection_cause,
                        reason: "peer-only turn delivery was rejected by the remote member"
                            .to_string(),
                    });
                }
                let peer = authorization.peer;
                let authority = self.supervisor_bridge.authority().await;
                let sup_spec = self
                    .supervisor_bridge
                    .supervisor_spec_for_recipient(&peer)
                    .await?;
                let command = super::bridge_protocol::BridgeCommand::DeliverMemberInput(
                    plain_delivery_payload(
                        sup_spec.into(),
                        authority.epoch,
                        authority.protocol_version,
                        meerkat_core::time_compat::new_uuid_v7().to_string(),
                        None,
                        None,
                        None,
                        &req,
                    ),
                );
                // DEC-P6E-19: surface the request envelope id — the member
                // stamps it as the accepted input's InteractionId, so the
                // pumped interaction terminals carry it and the remote
                // completion waiter can resolve on it.
                let (response, envelope_id): (
                    super::bridge_protocol::BridgeDeliveryResponse,
                    uuid::Uuid,
                ) = self
                    .send_bridge_command_typed_with_receipt(&peer, &command, Duration::from_secs(5))
                    .await?;
                match response.outcome {
                    super::bridge_protocol::BridgeDeliveryOutcome::Accepted
                    | super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated { .. } => {}
                    super::bridge_protocol::BridgeDeliveryOutcome::Rejected { cause, reason } => {
                        return Err(MobError::BridgeDeliveryRejected {
                            cause: Box::new(cause),
                            reason,
                        });
                    }
                }
                if let Err(error) = self
                    .session
                    .ops_adapter
                    .report_member_progress(member_ref, "turn dispatched")
                    .await
                {
                    tracing::warn!(
                        member_ref = ?member_ref,
                        error = %error,
                        "turn was accepted but controller progress bookkeeping failed"
                    );
                }
                Ok(Some(envelope_id.to_string()))
            }
            _ => self
                .session
                .start_turn(member_ref, req)
                .await
                .map(|()| None),
        }
    }

    async fn start_turn_with_correlation(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
        placed: Option<PlacedTurnDeliveryContext>,
    ) -> Result<Option<String>, MobError> {
        let Some(placed) = placed else {
            // Explicitly preserves the two non-placed realizations:
            // Session-backed members stay local, and BackendPeer(None) is
            // the legacy peer-only bridge lane.
            return self.start_turn_with_receipt(member_ref, req).await;
        };
        let MemberRef::BackendPeer {
            peer_id,
            address,
            pubkey,
            bootstrap_token,
            session_id: route_session_id,
        } = member_ref
        else {
            return Err(MobError::Internal(
                "placed turn context requires a BackendPeer transport route".to_string(),
            ));
        };
        validate_placed_delivery_context_ids(&placed)?;
        if !placed_route_session_matches_machine(
            route_session_id.as_ref(),
            &placed.expected_member.member_session_id,
        ) {
            let route_session = route_session_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "<sessionless>".to_string());
            return Err(MobError::Internal(format!(
                "placed turn context session '{}' does not match member ref session '{}'",
                placed.expected_member.member_session_id, route_session
            )));
        }
        if req.event_tx.is_some() {
            return Err(MobError::UnsupportedForMode {
                mode: crate::MobRuntimeMode::TurnDriven,
                reason: "tracked turn event streams are not supported for placed members"
                    .to_string(),
            });
        }
        let peer = Self::peer_only_spec_from_parts(peer_id, address, *pubkey)?;
        let authorization = self
            .ensure_supervisor_authorized(
                &peer,
                Some((
                    peer_id.as_str(),
                    address.as_str(),
                    bootstrap_token
                        .as_ref()
                        .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                    *pubkey,
                )),
                None,
            )
            .await?;
        if let Some(observation) = authorization.rebind_required {
            return Err(MobError::BridgeCommandRejected {
                cause: observation.rejection_cause,
                reason: "placed turn delivery was rejected by the remote member".to_string(),
            });
        }
        let peer = authorization.peer;
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self
            .supervisor_bridge
            .supervisor_spec_for_recipient(&peer)
            .await?;
        let command =
            super::bridge_protocol::BridgeCommand::DeliverMemberInput(plain_delivery_payload(
                sup_spec.into(),
                authority.epoch,
                super::bridge_protocol::BridgeProtocolVersion::V4,
                placed.input_id.clone(),
                placed.transcript_interaction_id.clone(),
                Some(placed.expected_member.clone()),
                placed.outcome_tracking,
                &req,
            ));
        // First response loss is ambiguous: resend the byte-identical command
        // once. The payload input_id, not either transport envelope id, is the
        // runtime InputId and waiter correlation.
        let response: super::bridge_protocol::BridgeDeliveryResponse = match self
            .send_bridge_command_typed(&peer, &command, Duration::from_secs(5))
            .await
        {
            Ok(response) => response,
            Err(error) if idempotent_bridge_resend_class(&error) => {
                match self
                    .send_bridge_command_typed(&peer, &command, Duration::from_secs(5))
                    .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        return Err(deliver_member_input_error_to_delivery_semantics(error));
                    }
                }
            }
            Err(error) => {
                return Err(deliver_member_input_error_to_delivery_semantics(error));
            }
        };
        validate_placed_plain_delivery_response(&placed.input_id, &response)?;
        match response.outcome {
            super::bridge_protocol::BridgeDeliveryOutcome::Accepted
            | super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated { .. } => {}
            super::bridge_protocol::BridgeDeliveryOutcome::Rejected { cause, reason } => {
                return Err(MobError::BridgeDeliveryRejected {
                    cause: Box::new(cause),
                    reason,
                });
            }
        }
        // Operation progress remains controller-owned, but it is keyed by the
        // exact placed peer binding (`MemberOpsKey::BackendPeer`) installed at
        // materialization. `MobOpsAdapter` deliberately ignores the projected
        // remote session copy and validates the pre-minted owner-session
        // operation before reporting progress.
        if let Err(error) = self
            .session
            .ops_adapter
            .report_member_progress(member_ref, "turn dispatched")
            .await
        {
            tracing::warn!(
                member_ref = ?member_ref,
                input_id = %placed.input_id,
                error = %error,
                "placed turn was accepted but controller progress bookkeeping failed"
            );
        }
        Ok(Some(placed.input_id))
    }

    async fn deliver_turn_directive(
        &self,
        member_ref: &MemberRef,
        request: super::remote_flow_ticket::TurnDirectiveDelivery,
    ) -> Result<super::remote_flow_ticket::BridgeDeliveryOutcomeReceipt, MobError> {
        // The machine classified this dispatch RemoteTurnDirective (the
        // placement fact); the ref only supplies the transport tuple. Its
        // optional session copy is irrelevant to the send (ADJ-24).
        let MemberRef::BackendPeer {
            peer_id,
            address,
            pubkey,
            bootstrap_token,
            ..
        } = member_ref
        else {
            // A non-peer ref here is a wiring fault, never a fallback path.
            return Err(MobError::Internal(format!(
                "turn directive dispatched for a member without a placed peer binding \
                 (member ref: {member_ref:?})"
            )));
        };
        let peer = Self::peer_only_spec_from_parts(peer_id, address, *pubkey)?;
        let authorization = self
            .ensure_supervisor_authorized(
                &peer,
                Some((
                    peer_id.as_str(),
                    address.as_str(),
                    bootstrap_token
                        .as_ref()
                        .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                    *pubkey,
                )),
                None,
            )
            .await?;
        if let Some(observation) = authorization.rebind_required {
            return Err(MobError::BridgeCommandRejected {
                cause: observation.rejection_cause,
                reason: "turn directive delivery was rejected by the remote member".to_string(),
            });
        }
        let peer = authorization.peer;
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self
            .supervisor_bridge
            .supervisor_spec_for_recipient(&peer)
            .await?;
        // The CALLER minted input_id: the obligation / journal / ticket key
        // (DEC-P6F-4). The resend below reuses the byte-identical command at
        // the SAME id — member dedup + journal replay both converge.
        let command = super::bridge_protocol::BridgeCommand::DeliverMemberInput(
            super::bridge_protocol::BridgeDeliveryPayload {
                supervisor: sup_spec.into(),
                epoch: authority.epoch,
                protocol_version: super::bridge_protocol::BridgeProtocolVersion::V4,
                input_id: request.input_id.clone(),
                transcript_interaction_id: None,
                content: request.content.clone(),
                handling_mode: request.handling_mode,
                objective_id: None,
                expected_member: Some(request.expected_member.clone()),
                // Flow steps carry no supervisor-attached injected context;
                // the member-side admission rejects a non-empty carrier.
                injected_context: Vec::new(),
                turn: Some(request.directive.clone()),
                outcome_tracking: None,
            },
        );
        // ADJ-4 resend class: exactly ONE resend, only on send-timeout /
        // wire Unavailable (the materialize precedent).
        let response: super::bridge_protocol::BridgeDeliveryResponse = match self
            .send_bridge_command_typed(&peer, &command, TURN_DIRECTIVE_DELIVERY_TIMEOUT)
            .await
        {
            Ok(response) => response,
            Err(error) if idempotent_bridge_resend_class(&error) => {
                match self
                    .send_bridge_command_typed(&peer, &command, TURN_DIRECTIVE_DELIVERY_TIMEOUT)
                    .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        return Err(deliver_member_input_error_to_delivery_semantics(error));
                    }
                }
            }
            Err(error) => {
                return Err(deliver_member_input_error_to_delivery_semantics(error));
            }
        };
        validate_turn_directive_delivery_response(&request.input_id, &response)?;
        match response.outcome {
            outcome @ (super::bridge_protocol::BridgeDeliveryOutcome::Accepted
            | super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated { .. }) => {
                Ok(super::remote_flow_ticket::BridgeDeliveryOutcomeReceipt {
                    input_id: request.input_id,
                    canonical_input_id: response.canonical_input_id,
                    outcome,
                })
            }
            super::bridge_protocol::BridgeDeliveryOutcome::Rejected { cause, reason } => {
                Err(MobError::BridgeDeliveryRejected {
                    cause: Box::new(cause),
                    reason,
                })
            }
        }
    }

    async fn admit_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => self.start_turn(member_ref, req).await,
            _ => self.session.admit_turn(member_ref, req).await,
        }
    }

    async fn admit_tracked_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
        completion_tx: tokio::sync::oneshot::Sender<Result<(), MobError>>,
        llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer { .. } => Err(MobError::UnsupportedForMode {
                mode: crate::MobRuntimeMode::TurnDriven,
                reason:
                    "tracked completion is not supported for remotely hosted or peer-only members"
                        .to_string(),
            }),
            MemberRef::Session { .. } => {
                self.session
                    .admit_tracked_turn(member_ref, req, completion_tx, llm_identity_applied_tx)
                    .await
            }
        }
    }

    async fn admit_turn_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => self.start_turn(member_ref, req).await,
            _ => {
                self.session
                    .admit_turn_for_operation(member_ref, operation_id, req)
                    .await
            }
        }
    }

    async fn reconcile_peer_only_trust(
        &self,
        member_ref: &MemberRef,
        desired_trust: Option<&PeerOnlyTrustOverlay>,
        rebind_authority: Option<&PeerOnlyRebindAuthority>,
    ) -> Result<PeerOnlyTrustReconcileReport, MobError> {
        let MemberRef::BackendPeer {
            peer_id,
            address,
            pubkey,
            bootstrap_token,
            session_id: None,
            ..
        } = member_ref
        else {
            return Ok(PeerOnlyTrustReconcileReport::default());
        };

        let peer = self.peer_only_spec(member_ref).await?;
        let authorization = self
            .ensure_supervisor_authorized(
                &peer,
                Some((
                    peer_id.as_str(),
                    address.as_str(),
                    bootstrap_token
                        .as_ref()
                        .map(super::bridge_protocol::BridgeBootstrapToken::as_str),
                    *pubkey,
                )),
                rebind_authority,
            )
            .await?;
        let report = PeerOnlyTrustReconcileReport {
            rebind_required: authorization.rebind_required,
        };
        if report.rebind_required.is_some() {
            return Ok(report);
        }
        let Some(desired_trust) = desired_trust else {
            return Ok(report);
        };
        if desired_trust.is_empty() {
            return Ok(report);
        }
        let peer = authorization.peer;
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self
            .supervisor_bridge
            .supervisor_spec_for_recipient(&peer)
            .await?;
        let mob_peer_overlay = desired_trust.bridge_handoff();
        for desired_peer in desired_trust.peers() {
            let command = super::bridge_protocol::BridgeCommand::WireMember(
                super::bridge_protocol::BridgePeerWiringPayload {
                    supervisor: sup_spec.clone().into(),
                    epoch: authority.epoch,
                    protocol_version: authority.protocol_version,
                    peer_spec: desired_peer.clone().into(),
                    mob_peer_overlay: Some(mob_peer_overlay.clone()),
                },
            );
            let _ack: super::bridge_protocol::BridgeAck = self
                .send_bridge_command_typed(&peer, &command, Duration::from_secs(5))
                .await?;
        }
        Ok(report)
    }

    async fn interaction_event_injector(
        &self,
        bridge_session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.session
            .interaction_event_injector(bridge_session_id)
            .await
    }

    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => Ok(None),
            _ => self.session.is_member_active(member_ref).await,
        }
    }

    async fn prepare_member_session_for_explicit_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, MobError> {
        self.session
            .retire_exact_attachment_for_explicit_resume(session_id)
            .await
    }

    async fn ensure_runtime_session_state(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => Ok(()),
            _ => self.session.ensure_runtime_session_state(member_ref).await,
        }
    }

    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => None,
            _ => self.session.comms_runtime(member_ref).await,
        }
    }

    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        match member_ref {
            MemberRef::Session { .. } => {
                self.session
                    .trusted_peer_spec(member_ref, fallback_name, fallback_peer_id)
                    .await
            }
            MemberRef::BackendPeer { session_id, .. } => {
                if let Some(session_id) = session_id {
                    // External members keep a local bridge session for lifecycle
                    // transport (notifications, kickoff events). The trust spec
                    // uses the bridge session's comms identity — NOT the real
                    // external peer_id — because the bridge signs lifecycle
                    // messages with its own keypair. The caller-provided
                    // `fallback_peer_id` is the bridge's `comms.public_key()`,
                    // set by `do_wire`'s key resolution path.
                    return self
                        .session
                        .trusted_peer_spec(
                            &MemberRef::Session {
                                session_id: session_id.clone(),
                            },
                            fallback_name,
                            fallback_peer_id,
                        )
                        .await;
                }
                // No bridge — use the real BackendPeer identity directly.
                let mut spec = self.peer_only_spec(member_ref).await?;
                spec.name = meerkat_core::comms::PeerName::new(fallback_name.to_string()).map_err(
                    |error| MobError::WiringError(format!("invalid peer name: {error}")),
                )?;
                Ok(spec)
            }
        }
    }

    async fn trusted_peer_spec_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        match member_ref {
            MemberRef::Session { .. } => {
                self.session
                    .trusted_peer_spec_for_operation(
                        member_ref,
                        operation_id,
                        fallback_name,
                        fallback_peer_id,
                    )
                    .await
            }
            MemberRef::BackendPeer { session_id, .. } => {
                if let Some(session_id) = session_id {
                    return self
                        .session
                        .trusted_peer_spec_for_operation(
                            &MemberRef::Session {
                                session_id: session_id.clone(),
                            },
                            operation_id,
                            fallback_name,
                            fallback_peer_id,
                        )
                        .await;
                }
                let mut spec = self.peer_only_spec(member_ref).await?;
                spec.name = meerkat_core::comms::PeerName::new(fallback_name.to_string()).map_err(
                    |error| MobError::WiringError(format!("invalid peer name: {error}")),
                )?;
                Ok(spec)
            }
        }
    }

    async fn publish_trusted_peer_spec_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        trusted_peer: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::Session { .. } => {
                self.session
                    .publish_trusted_peer_spec_for_operation(member_ref, operation_id, trusted_peer)
                    .await
            }
            MemberRef::BackendPeer {
                session_id: Some(session_id),
                ..
            } => {
                self.session
                    .publish_trusted_peer_spec_for_operation(
                        &MemberRef::Session {
                            session_id: session_id.clone(),
                        },
                        operation_id,
                        trusted_peer,
                    )
                    .await
            }
            MemberRef::BackendPeer {
                session_id: None, ..
            } => Err(MobError::Internal(
                "cannot publish session operation peer readiness for a peer-only member"
                    .to_string(),
            )),
        }
    }

    async fn active_operation_id_for_member(&self, member_ref: &MemberRef) -> Option<OperationId> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => {
                self.session
                    .ops_adapter
                    .active_operation_id_for_member(member_ref)
                    .await
            }
            _ => {
                let bridge_session_id = member_ref.bridge_session_id()?;
                self.session
                    .ops_adapter
                    .active_operation_id_for_session(bridge_session_id)
                    .await
            }
        }
    }

    async fn bind_member_owner_context(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                session_id: None,
                ..
            } => {
                let operation_source = OperationSource::backend_peer(
                    meerkat_core::comms::PeerId::parse(peer_id).map_err(|error| {
                        MobError::Internal(format!(
                            "peer-only member operation source has invalid peer id '{peer_id}': {error}"
                        ))
                    })?,
                    meerkat_core::comms::PeerAddress::parse(address).map_err(|error| {
                        MobError::Internal(format!(
                            "peer-only member operation source has invalid address '{address}': {error}"
                        ))
                    })?,
                );
                let display_name = format!("mob_member/backend_peer/{peer_id}@{address}");
                self.session.ops_adapter.bind_member_registry(
                    member_ref,
                    owner_bridge_session_id,
                    ops_registry,
                    display_name.clone(),
                    operation_source,
                )?;
                let _ = self
                    .session
                    .ops_adapter
                    .mark_member_provisioned_for_member(member_ref, &display_name)
                    .await?;
                Ok(())
            }
            _ => {
                self.session
                    .bind_member_owner_context(member_ref, owner_bridge_session_id, ops_registry)
                    .await
            }
        }
    }

    async fn bind_placed_member_owner_context_exact(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
        display_name: String,
        provision_operation_id: OperationId,
        recovery_expectation: PlacedOperationRecoveryExpectation,
    ) -> Result<(), MobError> {
        let MemberRef::BackendPeer {
            peer_id,
            address,
            session_id: None,
            ..
        } = member_ref
        else {
            return Err(MobError::Internal(
                "exact placed operation binding requires a peer-shaped member without a local session alias"
                    .to_string(),
            ));
        };
        let operation_source = OperationSource::backend_peer(
            meerkat_core::comms::PeerId::parse(peer_id).map_err(|error| {
                MobError::Internal(format!(
                    "placed member operation source has invalid peer id '{peer_id}': {error}"
                ))
            })?,
            meerkat_core::comms::PeerAddress::parse(address).map_err(|error| {
                MobError::Internal(format!(
                    "placed member operation source has invalid address '{address}': {error}"
                ))
            })?,
        );
        super::ops_adapter::MobOpsAdapter::ensure_committed_placed_provision_operation_exact_for_recovery(
            ops_registry.as_ref(),
            &owner_bridge_session_id,
            &provision_operation_id,
            &display_name,
            recovery_expectation,
        )?;
        self.session
            .ops_adapter
            .bind_member_registry_for_exact_operation(
                member_ref,
                owner_bridge_session_id.clone(),
                ops_registry,
                display_name.clone(),
                operation_source,
                provision_operation_id.clone(),
            )?;
        self.session
            .ops_adapter
            .verify_running_placed_member_operation_exact(
                member_ref,
                &owner_bridge_session_id,
                &display_name,
                &provision_operation_id,
            )
            .await
    }

    async fn cancel_all_checkpointers(&self) {
        self.session.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.session.rearm_all_checkpointers().await;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod bridge_rejection_tests {
    use super::{
        MultiBackendProvisioner, TRACKED_INPUT_CANCEL_BRIDGE_TIMEOUT,
        deliver_member_input_error_to_delivery_semantics, idempotent_bridge_resend_class,
        placed_route_session_matches_machine, plain_delivery_payload,
        validate_materialized_member_principal_disjointness, validate_placed_delivery_context_ids,
        validate_placed_plain_delivery_response, validate_tracked_input_cancel_response,
        validate_turn_directive_delivery_response,
    };
    use crate::MobError;
    use crate::runtime::bridge_protocol::{
        BridgeRejectionCause, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        decode_legacy_v1_raw_string_rejection,
    };

    #[test]
    fn tracked_input_cancel_transport_budget_exceeds_host_settle_budget() {
        assert!(
            TRACKED_INPUT_CANCEL_BRIDGE_TIMEOUT > std::time::Duration::from_secs(5),
            "the controller transport envelope must outlive the host's cancellation settle window"
        );
    }
    use serde_json::json;

    #[test]
    fn materialized_member_peer_must_be_distinct_from_host_and_supervisor() {
        validate_materialized_member_principal_disjointness(
            "mob/profile/member",
            "member-peer",
            "host-peer",
            "supervisor-peer",
        )
        .expect("distinct trust principals are valid");
        for (member_peer, expected) in [
            ("host-peer", "host-peer"),
            ("supervisor-peer", "supervisor-peer"),
        ] {
            let error = validate_materialized_member_principal_disjointness(
                "mob/profile/member",
                member_peer,
                "host-peer",
                "supervisor-peer",
            )
            .expect_err("member alias must fail closed");
            assert!(error.to_string().contains(expected));
        }
    }

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
    fn provisioner_decodes_typed_protocol_v2_bridge_rejection() {
        let value = json!({
            "result": "rejected",
            "cause": "not_bound",
            "reason": "bind required",
        });

        let rejection = MultiBackendProvisioner::bridge_rejection_reply(
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            &value,
        )
        .expect("typed rejection should decode");

        assert_eq!(
            rejection.typed_cause(),
            Some(BridgeRejectionCause::NotBound)
        );
        assert_eq!(rejection.reason(), "bind required");
    }

    #[test]
    fn provisioner_does_not_promote_raw_string_as_protocol_v2_rejection() {
        let value = json!("legacy rejection");

        assert!(
            MultiBackendProvisioner::bridge_rejection_reply(
                SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                &value,
            )
            .is_none()
        );
        let legacy = decode_legacy_v1_raw_string_rejection(&value)
            .expect("legacy raw string should decode only through the explicit v1 helper");
        assert_eq!(legacy.typed_cause(), None);
        assert!(legacy.is_legacy_v1_raw_string());
    }

    #[test]
    fn provisioner_bridge_rejection_error_preserves_each_known_typed_cause() {
        for (cause, wire_name) in known_bridge_rejection_causes() {
            let reason = format!("typed bridge rejection: {wire_name}");
            let value = json!({
                "result": "rejected",
                "cause": wire_name,
                "reason": reason,
            });
            let rejection = MultiBackendProvisioner::bridge_rejection_reply(
                SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                &value,
            )
            .expect("typed rejection should decode");

            let error = MultiBackendProvisioner::bridge_rejection_error(rejection);

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
    fn provisioner_legacy_bridge_rejection_error_stays_untyped() {
        let value = json!("legacy rejection");
        let legacy = decode_legacy_v1_raw_string_rejection(&value)
            .expect("legacy raw string should decode only through the explicit v1 helper");

        let error = MultiBackendProvisioner::bridge_rejection_error(legacy);

        assert!(matches!(
            error,
            MobError::WiringError(reason) if reason == "legacy rejection"
        ));
    }

    #[test]
    fn deliver_member_input_proven_pre_admission_rejections_are_certified_no_effect() {
        let causes = [
            BridgeRejectionCause::NotBound,
            BridgeRejectionCause::StaleSupervisor,
            BridgeRejectionCause::SenderMismatch,
            BridgeRejectionCause::InvalidSupervisorSpec,
            BridgeRejectionCause::Unsupported,
        ];

        for cause in causes {
            let expected_unsupported = matches!(
                cause,
                BridgeRejectionCause::UnsupportedProtocolVersion
                    | BridgeRejectionCause::Unsupported
            );
            let expected_detail = serde_json::to_string(&cause).expect("wire cause serializes");
            let translated =
                deliver_member_input_error_to_delivery_semantics(MobError::BridgeCommandRejected {
                    cause,
                    reason: "receiver rejected before admission".to_string(),
                });
            match translated {
                MobError::BridgeDeliveryRejected { cause, reason } => {
                    assert_eq!(reason, "receiver rejected before admission");
                    match cause.as_ref() {
                        super::super::bridge_protocol::BridgeDeliveryRejectionCause::TurnDirectiveUnsupported {
                            detail,
                        } if expected_unsupported => assert!(detail.contains(&expected_detail)),
                        super::super::bridge_protocol::BridgeDeliveryRejectionCause::Internal {
                            detail,
                        } if !expected_unsupported => assert!(detail.contains(&expected_detail)),
                        other => panic!(
                            "unexpected delivery semantic for outer cause {expected_detail}: {other:?}"
                        ),
                    }
                }
                other => panic!("command rejection was not translated: {other:?}"),
            }
        }
    }

    #[test]
    fn deliver_member_input_old_protocol_rejection_is_certified_no_effect() {
        let translated =
            deliver_member_input_error_to_delivery_semantics(MobError::BridgeCommandRejected {
                cause: BridgeRejectionCause::UnsupportedProtocolVersion,
                reason: "V4 payload is unsupported".to_string(),
            });
        assert!(matches!(
            translated,
            MobError::BridgeDeliveryRejected {
                cause,
                reason,
            } if matches!(
                cause.as_ref(),
                super::super::bridge_protocol::BridgeDeliveryRejectionCause::TurnDirectiveUnsupported { .. }
            ) && reason == "V4 payload is unsupported"
        ));
    }

    #[test]
    fn deliver_member_input_ambiguous_outer_rejections_stay_ambiguous() {
        for cause in [
            BridgeRejectionCause::Internal,
            BridgeRejectionCause::Unavailable,
            BridgeRejectionCause::LiveTransportUnavailable,
            BridgeRejectionCause::StaleFence,
            BridgeRejectionCause::AlreadyBound,
            BridgeRejectionCause::CapabilityMissing {
                capability: "directed_turns".to_string(),
            },
        ] {
            let expected = cause.clone();
            assert!(matches!(
                deliver_member_input_error_to_delivery_semantics(
                    MobError::BridgeCommandRejected {
                        cause,
                        reason: "effect status is not certified".to_string(),
                    }
                ),
                MobError::BridgeCommandRejected { cause, reason }
                    if cause == expected && reason == "effect status is not certified"
            ));
        }
    }

    #[test]
    fn deliver_member_input_unavailable_is_resend_class_but_not_no_effect_proof() {
        let unavailable = MobError::BridgeCommandRejected {
            cause: BridgeRejectionCause::Unavailable,
            reason: "temporarily unavailable".to_string(),
        };
        assert!(
            idempotent_bridge_resend_class(&unavailable),
            "the first Unavailable reply still causes the exact one resend"
        );
        assert!(matches!(
            deliver_member_input_error_to_delivery_semantics(unavailable),
            MobError::BridgeCommandRejected {
                cause: BridgeRejectionCause::Unavailable,
                ..
            }
        ));
    }

    #[test]
    fn turn_directive_response_rejects_mismatched_request_id() {
        let response = super::super::bridge_protocol::BridgeDeliveryResponse {
            input_id: "other".to_string(),
            canonical_input_id: Some("canonical".to_string()),
            outcome: super::super::bridge_protocol::BridgeDeliveryOutcome::Accepted,
        };
        assert!(matches!(
            validate_turn_directive_delivery_response("requested", &response),
            Err(MobError::Internal(reason))
                if reason.contains("malformed authenticated turn-directive response")
        ));
    }

    #[test]
    fn turn_directive_response_rejects_missing_or_inconsistent_canonical_id() {
        let accepted = super::super::bridge_protocol::BridgeDeliveryResponse {
            input_id: "requested".to_string(),
            canonical_input_id: None,
            outcome: super::super::bridge_protocol::BridgeDeliveryOutcome::Accepted,
        };
        assert!(validate_turn_directive_delivery_response("requested", &accepted).is_err());

        let accepted_wrong_canonical = super::super::bridge_protocol::BridgeDeliveryResponse {
            input_id: "requested".to_string(),
            canonical_input_id: Some("unrelated".to_string()),
            outcome: super::super::bridge_protocol::BridgeDeliveryOutcome::Accepted,
        };
        assert!(
            validate_turn_directive_delivery_response("requested", &accepted_wrong_canonical)
                .is_err()
        );

        let deduplicated = super::super::bridge_protocol::BridgeDeliveryResponse {
            input_id: "requested".to_string(),
            canonical_input_id: Some("wrong".to_string()),
            outcome: super::super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated {
                existing_input_id: "canonical".to_string(),
            },
        };
        assert!(validate_turn_directive_delivery_response("requested", &deduplicated).is_err());

        let deduplicated_alias = super::super::bridge_protocol::BridgeDeliveryResponse {
            input_id: "requested".to_string(),
            canonical_input_id: Some("unrelated".to_string()),
            outcome: super::super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated {
                existing_input_id: "unrelated".to_string(),
            },
        };
        assert!(
            validate_turn_directive_delivery_response("requested", &deduplicated_alias).is_err()
        );

        let rejected = super::super::bridge_protocol::BridgeDeliveryResponse {
            input_id: "requested".to_string(),
            canonical_input_id: Some("contradictory".to_string()),
            outcome: super::super::bridge_protocol::BridgeDeliveryOutcome::Rejected {
                cause: super::super::bridge_protocol::BridgeDeliveryRejectionCause::NotReady {
                    state: super::super::bridge_protocol::BridgeMemberRuntimeState::Idle,
                },
                reason: "not accepted".to_string(),
            },
        };
        assert!(validate_turn_directive_delivery_response("requested", &rejected).is_err());
    }

    #[test]
    fn placed_plain_delivery_accept_and_dedup_preserve_stable_payload_uuid() {
        let input_id = uuid::Uuid::new_v4().to_string();
        let accepted = super::super::bridge_protocol::BridgeDeliveryResponse {
            input_id: input_id.clone(),
            canonical_input_id: Some(input_id.clone()),
            outcome: super::super::bridge_protocol::BridgeDeliveryOutcome::Accepted,
        };
        validate_placed_plain_delivery_response(&input_id, &accepted)
            .expect("first accepted response retains the payload UUID");

        // This is the response to the byte-identical resend after the first
        // response was lost. It must address the same runtime input/waiter.
        let deduplicated = super::super::bridge_protocol::BridgeDeliveryResponse {
            input_id: input_id.clone(),
            canonical_input_id: Some(input_id.clone()),
            outcome: super::super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated {
                existing_input_id: input_id.clone(),
            },
        };
        validate_placed_plain_delivery_response(&input_id, &deduplicated)
            .expect("deduplicated retry retains the same payload UUID");

        let wrong = super::super::bridge_protocol::BridgeDeliveryResponse {
            input_id: input_id.clone(),
            canonical_input_id: Some(uuid::Uuid::new_v4().to_string()),
            outcome: super::super::bridge_protocol::BridgeDeliveryOutcome::Accepted,
        };
        assert!(validate_placed_plain_delivery_response(&input_id, &wrong).is_err());
    }

    #[test]
    fn placed_route_session_defers_to_machine_authority_when_route_is_sessionless() {
        assert!(placed_route_session_matches_machine(
            None,
            "machine-session"
        ));
    }

    #[test]
    fn placed_route_session_rejects_a_present_conflicting_session() {
        let route_session = meerkat_core::types::SessionId::new();
        assert!(!placed_route_session_matches_machine(
            Some(&route_session),
            "different-machine-session",
        ));
    }

    #[test]
    fn tracked_cancel_response_is_exact_to_residency_key_and_terminal() {
        let expected_member = super::super::bridge_protocol::BridgeMemberIncarnation {
            mob_id: "mob-1".to_string(),
            agent_identity: "worker-1".to_string(),
            host_id: "host-a".to_string(),
            binding_generation: 2,
            member_session_id: "session-a".to_string(),
            generation: 3,
            fence_token: 4,
        };
        let input_id = uuid::Uuid::from_u128(301).to_string();
        let exact = super::super::bridge_protocol::BridgeTrackedInputCancelResponse {
            expected_member: expected_member.clone(),
            input_id: input_id.clone(),
            outcome: super::super::bridge_protocol::BridgeTrackedInputCancelOutcome::Terminal {
                record: super::super::bridge_protocol::BridgeTurnOutcomeRecord {
                    input_id: input_id.clone(),
                    generation: expected_member.generation,
                    fence_token: expected_member.fence_token,
                    terminal_seq: 9,
                    outcome:
                        super::super::bridge_protocol::WireFlowTurnOutcome::InteractionComplete,
                },
            },
        };
        validate_tracked_input_cancel_response(&expected_member, &input_id, &exact)
            .expect("exact tracked cancellation response validates");

        let moved = super::super::bridge_protocol::BridgeTrackedInputCancelResponse {
            expected_member: super::super::bridge_protocol::BridgeMemberIncarnation {
                member_session_id: "session-b".to_string(),
                ..expected_member.clone()
            },
            ..exact.clone()
        };
        assert!(
            validate_tracked_input_cancel_response(&expected_member, &input_id, &moved).is_err()
        );

        let crossed_terminal = super::super::bridge_protocol::BridgeTrackedInputCancelResponse {
            outcome: super::super::bridge_protocol::BridgeTrackedInputCancelOutcome::Terminal {
                record: super::super::bridge_protocol::BridgeTurnOutcomeRecord {
                    input_id: uuid::Uuid::from_u128(302).to_string(),
                    generation: expected_member.generation,
                    fence_token: expected_member.fence_token,
                    terminal_seq: 9,
                    outcome:
                        super::super::bridge_protocol::WireFlowTurnOutcome::InteractionComplete,
                },
            },
            ..exact
        };
        assert!(
            validate_tracked_input_cancel_response(&expected_member, &input_id, &crossed_terminal,)
                .is_err()
        );
    }

    #[test]
    fn placed_delivery_payload_preserves_objective_incarnation_and_tracking() {
        let objective_id = meerkat_core::interaction::ObjectiveId::new();
        let input_id = uuid::Uuid::new_v4().to_string();
        let expected_member = super::super::bridge_protocol::BridgeMemberIncarnation {
            mob_id: "mob-1".to_string(),
            agent_identity: "worker-1".to_string(),
            host_id: "host-b".to_string(),
            binding_generation: 7,
            member_session_id: "session-1".to_string(),
            generation: 2,
            fence_token: 3,
        };
        let placed = super::PlacedTurnDeliveryContext {
            input_id: input_id.clone(),
            transcript_interaction_id: Some(input_id),
            expected_member: expected_member.clone(),
            outcome_tracking: Some(
                super::super::bridge_protocol::BridgeOutcomeTracking::Interaction,
            ),
        };
        let request = meerkat_core::service::StartTurnRequest {
            injected_context: Vec::new(),
            prompt: meerkat_core::types::ContentInput::Text("kickoff".to_string()),
            system_prompt: None,
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::runtime_metadata(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    transcript_identity: meerkat_core::types::TranscriptMessageIdentity {
                        interaction_id: None,
                        run_id: None,
                        objective_id: Some(objective_id),
                    },
                    ..Default::default()
                },
            ),
        };

        let payload = plain_delivery_payload(
            super::super::bridge_protocol::BridgePeerSpec {
                name: "supervisor".to_string(),
                peer_id: uuid::Uuid::new_v4().to_string(),
                address: "inproc://supervisor".to_string(),
                pubkey: [9; 32],
            },
            11,
            super::super::bridge_protocol::BridgeProtocolVersion::V4,
            placed.input_id.clone(),
            placed.transcript_interaction_id.clone(),
            Some(expected_member.clone()),
            placed.outcome_tracking,
            &request,
        );

        assert_eq!(payload.objective_id, Some(objective_id));
        assert_eq!(payload.input_id, placed.input_id);
        assert_eq!(
            payload.transcript_interaction_id,
            placed.transcript_interaction_id
        );
        assert_eq!(payload.expected_member, Some(expected_member));
        assert_eq!(
            payload.outcome_tracking,
            Some(super::super::bridge_protocol::BridgeOutcomeTracking::Interaction)
        );
        assert_eq!(payload.epoch, 11);
        assert_eq!(
            payload.protocol_version,
            super::super::bridge_protocol::BridgeProtocolVersion::V4
        );
        assert!(payload.turn.is_none());
    }

    #[test]
    fn placed_delivery_context_rejects_invalid_or_split_terminal_identity() {
        let input_id = uuid::Uuid::new_v4().to_string();
        let base = super::PlacedTurnDeliveryContext {
            input_id: input_id.clone(),
            transcript_interaction_id: Some(input_id),
            expected_member: super::super::bridge_protocol::BridgeMemberIncarnation {
                mob_id: "mob".to_string(),
                agent_identity: "worker".to_string(),
                host_id: "host".to_string(),
                binding_generation: 1,
                member_session_id: "session".to_string(),
                generation: 1,
                fence_token: 1,
            },
            outcome_tracking: Some(
                super::super::bridge_protocol::BridgeOutcomeTracking::Interaction,
            ),
        };
        validate_placed_delivery_context_ids(&base).expect("exact tracked identity validates");

        let split = super::PlacedTurnDeliveryContext {
            transcript_interaction_id: Some(uuid::Uuid::new_v4().to_string()),
            ..base.clone()
        };
        assert!(validate_placed_delivery_context_ids(&split).is_err());

        let nil = super::PlacedTurnDeliveryContext {
            input_id: uuid::Uuid::nil().to_string(),
            transcript_interaction_id: Some(uuid::Uuid::nil().to_string()),
            ..base.clone()
        };
        assert!(validate_placed_delivery_context_ids(&nil).is_err());

        let admission_without_caller_id = super::PlacedTurnDeliveryContext {
            input_id: uuid::Uuid::new_v4().to_string(),
            transcript_interaction_id: None,
            outcome_tracking: None,
            ..base
        };
        validate_placed_delivery_context_ids(&admission_without_caller_id)
            .expect("untracked admission may preserve absent transcript identity");
    }
}

#[cfg(any(feature = "runtime-adapter", test))]
#[allow(clippy::too_many_arguments)]
fn plain_delivery_payload(
    supervisor: super::bridge_protocol::BridgePeerSpec,
    epoch: u64,
    protocol_version: super::bridge_protocol::BridgeProtocolVersion,
    input_id: String,
    transcript_interaction_id: Option<String>,
    expected_member: Option<super::bridge_protocol::BridgeMemberIncarnation>,
    outcome_tracking: Option<super::bridge_protocol::BridgeOutcomeTracking>,
    req: &StartTurnRequest,
) -> super::bridge_protocol::BridgeDeliveryPayload {
    super::bridge_protocol::BridgeDeliveryPayload {
        supervisor,
        epoch,
        protocol_version,
        input_id,
        transcript_interaction_id,
        content: req.prompt.clone(),
        handling_mode: req.runtime.handling_mode,
        objective_id: turn_request_objective_id(req),
        expected_member,
        injected_context: req.injected_context.clone(),
        turn: None,
        outcome_tracking,
    }
}

#[cfg(any(feature = "runtime-adapter", test))]
fn turn_request_objective_id(
    req: &StartTurnRequest,
) -> Option<meerkat_core::interaction::ObjectiveId> {
    req.runtime
        .turn_metadata
        .as_ref()
        .and_then(|metadata| metadata.transcript_identity.objective_id)
}

#[cfg(any(feature = "runtime-adapter", test))]
fn placed_route_session_matches_machine(
    route_session_id: Option<&SessionId>,
    machine_session_id: &str,
) -> bool {
    route_session_id.is_none_or(|session_id| session_id.to_string() == machine_session_id)
}

#[cfg(any(feature = "runtime-adapter", test))]
fn validate_placed_delivery_context_ids(
    placed: &PlacedTurnDeliveryContext,
) -> Result<(), MobError> {
    let input_id = uuid::Uuid::parse_str(&placed.input_id)
        .map_err(|_| MobError::Internal("placed delivery input_id is not a UUID".to_string()))?;
    if input_id.is_nil() || input_id.to_string() != placed.input_id {
        return Err(MobError::Internal(
            "placed delivery input_id must be a canonical non-nil UUID".to_string(),
        ));
    }

    if let Some(transcript_id) = placed.transcript_interaction_id.as_deref() {
        let parsed = uuid::Uuid::parse_str(transcript_id).map_err(|_| {
            MobError::Internal(
                "placed delivery transcript_interaction_id is not a UUID".to_string(),
            )
        })?;
        if parsed.is_nil() || parsed.to_string() != transcript_id {
            return Err(MobError::Internal(
                "placed delivery transcript_interaction_id must be a canonical non-nil UUID"
                    .to_string(),
            ));
        }
    }

    if placed.outcome_tracking == Some(super::bridge_protocol::BridgeOutcomeTracking::Interaction)
        && placed.transcript_interaction_id.as_deref() != Some(placed.input_id.as_str())
    {
        return Err(MobError::Internal(
            "tracked placed completion requires transcript_interaction_id to equal input_id"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(any(feature = "runtime-adapter", test))]
fn validate_tracked_input_cancel_response(
    expected_member: &super::bridge_protocol::BridgeMemberIncarnation,
    input_id: &str,
    response: &super::bridge_protocol::BridgeTrackedInputCancelResponse,
) -> Result<(), MobError> {
    if response.expected_member != *expected_member || response.input_id != input_id {
        return Err(MobError::Internal(format!(
            "tracked input cancellation reply changed its exact residency/key: expected {expected_member:?}/{input_id}, got {:?}/{}",
            response.expected_member, response.input_id
        )));
    }
    if let super::bridge_protocol::BridgeTrackedInputCancelOutcome::Terminal { record } =
        &response.outcome
        && (record.input_id != input_id
            || record.generation != expected_member.generation
            || record.fence_token != expected_member.fence_token)
    {
        return Err(MobError::Internal(
            "tracked input cancellation terminal does not match the exact residency/key"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(any(feature = "runtime-adapter", test))]
fn validate_turn_directive_delivery_response(
    requested_input_id: &str,
    response: &super::bridge_protocol::BridgeDeliveryResponse,
) -> Result<(), MobError> {
    let malformed = |detail: String| {
        MobError::Internal(format!(
            "malformed authenticated turn-directive response: {detail}"
        ))
    };
    if response.input_id != requested_input_id {
        return Err(malformed(format!(
            "response input_id '{}' does not match request '{}'",
            response.input_id, requested_input_id
        )));
    }
    match &response.outcome {
        super::bridge_protocol::BridgeDeliveryOutcome::Accepted => {
            if response.canonical_input_id.as_deref() != Some(requested_input_id) {
                return Err(malformed(format!(
                    "Accepted canonical_input_id {:?} does not match requested input_id '{requested_input_id}'",
                    response.canonical_input_id
                )));
            }
        }
        super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated { existing_input_id } => {
            if response.canonical_input_id.as_deref() != Some(requested_input_id)
                || existing_input_id != requested_input_id
            {
                return Err(malformed(format!(
                    "Deduplicated ids canonical={:?}, existing='{}' do not both match requested input_id '{requested_input_id}'",
                    response.canonical_input_id, existing_input_id,
                )));
            }
        }
        super::bridge_protocol::BridgeDeliveryOutcome::Rejected { .. } => {
            if response.canonical_input_id.is_some() {
                return Err(malformed(
                    "Rejected response included canonical_input_id".to_string(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(any(feature = "runtime-adapter", test))]
fn validate_placed_plain_delivery_response(
    requested_input_id: &str,
    response: &super::bridge_protocol::BridgeDeliveryResponse,
) -> Result<(), MobError> {
    validate_turn_directive_delivery_response(requested_input_id, response)?;
    let malformed = |detail: String| {
        MobError::Internal(format!(
            "malformed authenticated placed-turn response: {detail}"
        ))
    };
    match &response.outcome {
        super::bridge_protocol::BridgeDeliveryOutcome::Accepted => {
            if response.canonical_input_id.as_deref() != Some(requested_input_id) {
                return Err(malformed(format!(
                    "accepted canonical_input_id {:?} does not match stable payload input_id '{requested_input_id}'",
                    response.canonical_input_id
                )));
            }
        }
        super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated { existing_input_id } => {
            if existing_input_id != requested_input_id {
                return Err(malformed(format!(
                    "deduplicated existing_input_id '{existing_input_id}' does not match stable payload input_id '{requested_input_id}'"
                )));
            }
        }
        super::bridge_protocol::BridgeDeliveryOutcome::Rejected { .. } => {}
    }
    Ok(())
}

/// ADJ-4 transient class: send-timeout and wire `Unavailable` only.
#[cfg(any(feature = "runtime-adapter", test))]
fn idempotent_bridge_resend_class(error: &MobError) -> bool {
    matches!(
        error,
        MobError::BridgeRequestTimedOut { .. }
            | MobError::BridgeCommandRejected {
                cause: super::bridge_protocol::BridgeRejectionCause::Unavailable,
                ..
            }
    )
}

/// Only a positive set of outer `BridgeReply::Rejected` causes proves that
/// `DeliverMemberInput` stopped before member-input admission. Their receiver
/// producers live exclusively in command decoding or supervisor admission:
/// old-protocol/invalid-command rejection and bound-supervisor parse/auth/phase
/// rejection. Translate those onto the delivery carrier so the controller may
/// close Pending custody as certified no-effect.
///
/// Outer `Internal` is deliberately not sufficient proof: the tracked receiver
/// uses it both before admission and after a runtime accept whose durable effect
/// may already exist. `Unavailable` still triggers ADJ-4's one exact resend,
/// but neither the first nor second reply certifies effect status. Unknown or
/// command-specific causes likewise stay raw/ambiguous rather than gaining
/// no-effect semantics by accident.
#[cfg(any(feature = "runtime-adapter", test))]
fn deliver_member_input_error_to_delivery_semantics(error: MobError) -> MobError {
    match error {
        MobError::BridgeCommandRejected { cause, reason }
            if matches!(
                cause,
                super::bridge_protocol::BridgeRejectionCause::NotBound
                    | super::bridge_protocol::BridgeRejectionCause::StaleSupervisor
                    | super::bridge_protocol::BridgeRejectionCause::SenderMismatch
                    | super::bridge_protocol::BridgeRejectionCause::UnsupportedProtocolVersion
                    | super::bridge_protocol::BridgeRejectionCause::InvalidSupervisorSpec
                    | super::bridge_protocol::BridgeRejectionCause::Unsupported
            ) =>
        {
            let cause_detail = serde_json::to_string(&cause)
                .unwrap_or_else(|_| format!("unserializable:{cause:?}"));
            let delivery_cause = match cause {
                super::bridge_protocol::BridgeRejectionCause::UnsupportedProtocolVersion
                | super::bridge_protocol::BridgeRejectionCause::Unsupported => {
                    super::bridge_protocol::BridgeDeliveryRejectionCause::TurnDirectiveUnsupported {
                        detail: format!(
                            "command-level DeliverMemberInput rejection before admission: {cause_detail}"
                        ),
                    }
                }
                _ => super::bridge_protocol::BridgeDeliveryRejectionCause::Internal {
                    detail: format!(
                        "command-level DeliverMemberInput rejection before admission: {cause_detail}"
                    ),
                },
            };
            MobError::BridgeDeliveryRejected {
                cause: Box::new(delivery_cause),
                reason,
            }
        }
        other => other,
    }
}

#[cfg(any(feature = "runtime-adapter", test))]
fn validate_materialized_member_principal_disjointness(
    peer_name: &str,
    member_peer_id: &str,
    host_peer_id: &str,
    supervisor_peer_id: &str,
) -> Result<(), MobError> {
    if member_peer_id == host_peer_id || member_peer_id == supervisor_peer_id {
        return Err(MobError::WiringError(format!(
            "materialize ack for '{peer_name}' aliases host/supervisor trust principal '{member_peer_id}'"
        )));
    }
    Ok(())
}

/// Decode an `ed25519:<base64>` member pubkey into its 32 raw bytes,
/// rejecting the zero key (fail closed — a zero-key member could never be
/// trust-validated).
#[cfg(feature = "runtime-adapter")]
fn decode_ack_member_pubkey(pubkey: &str) -> Result<[u8; 32], MobError> {
    let encoded = pubkey.strip_prefix("ed25519:").ok_or_else(|| {
        MobError::WiringError(format!(
            "materialize ack member_pubkey '{pubkey}' is not ed25519-prefixed"
        ))
    })?;
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let value_of = |c: u8| -> Result<u32, MobError> {
        ALPHABET
            .iter()
            .position(|&a| a == c)
            .map(|v| v as u32)
            .ok_or_else(|| {
                MobError::WiringError(
                    "materialize ack member_pubkey carries a non-base64 character".to_string(),
                )
            })
    };
    let stripped: Vec<u8> = encoded.bytes().filter(|&c| c != b'=').collect();
    let mut bytes = Vec::with_capacity(32);
    for chunk in stripped.chunks(4) {
        let mut n: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value_of(c)? << (18 - 6 * i as u32);
        }
        for i in 0..chunk.len().saturating_sub(1) {
            bytes.push(((n >> (16 - 8 * i as u32)) & 0xff) as u8);
        }
    }
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
        MobError::WiringError("materialize ack member_pubkey is not 32 bytes".to_string())
    })?;
    if bytes == [0u8; 32] {
        return Err(MobError::WiringError(
            "materialize ack member_pubkey is the zero key".to_string(),
        ));
    }
    Ok(bytes)
}
