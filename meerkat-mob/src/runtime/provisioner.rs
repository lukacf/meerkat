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
    CoreExecutorInterruptHandle,
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
#[cfg(feature = "runtime-adapter")]
use meerkat_core::time_compat::{Duration, Instant};
use meerkat_core::types::SessionId;
#[cfg(feature = "runtime-adapter")]
#[allow(unused_imports)]
use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
#[cfg(feature = "runtime-adapter")]
use meerkat_runtime::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, MeerkatMachine, PromptInput,
};
#[cfg(feature = "runtime-adapter")]
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};

type TurnEventTx = tokio::sync::mpsc::Sender<meerkat_core::EventEnvelope<meerkat_core::AgentEvent>>;

#[cfg(not(target_arch = "wasm32"))]
type ArchiveSessionFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SessionError>> + Send + 'a>>;

#[cfg(target_arch = "wasm32")]
type ArchiveSessionFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SessionError>> + 'a>>;

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
    release_tx: oneshot::Sender<()>,
}

#[cfg(feature = "runtime-adapter")]
impl DeferredTurnEventDelivery {
    fn release(self) {
        let _ = self.release_tx.send(());
    }
}

#[cfg(feature = "runtime-adapter")]
fn defer_turn_events_until_machine_completion(
    session_id: &SessionId,
    event_tx: Option<TurnEventTx>,
) -> (Option<TurnEventTx>, Option<DeferredTurnEventDelivery>) {
    let Some(event_tx) = event_tx else {
        return (None, None);
    };

    let (deferred_tx, mut deferred_rx) = mpsc::channel(DEFERRED_TURN_EVENT_CHANNEL_CAPACITY);
    let (release_tx, release_rx) = oneshot::channel();
    let session_id = session_id.clone();
    tokio::spawn(async move {
        let mut release_rx = Box::pin(release_rx);
        let mut buffered = Vec::new();
        let mut stream_closed = false;
        let mut released = false;

        loop {
            tokio::select! {
                event = deferred_rx.recv(), if !stream_closed => {
                    match event {
                        Some(event) => buffered.push(event),
                        None => stream_closed = true,
                    }
                }
                result = &mut release_rx, if !released => {
                    released = true;
                    drop(result);
                }
            }

            if stream_closed && released {
                break;
            }
        }

        for event in buffered {
            if event_tx.send(event).await.is_err() {
                return;
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
    /// Whether provisioning owns a newly-created session or is temporarily
    /// attaching an existing durable session. Compensation is destructive
    /// only for the former.
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobProvisioner: Send + Sync {
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
    /// Release a failed resume provision without archiving its durable
    /// session. The runtime is returned to durable idle and detached from the
    /// failed member incarnation.
    async fn restore_resumed_member(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        original_origin: ProvisionSessionOrigin,
    ) -> Result<(), MobError>;
    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
    async fn hard_cancel_member(
        &self,
        member_ref: &MemberRef,
        reason: &str,
    ) -> Result<(), MobError>;
    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError>;
    async fn admit_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError>;
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

    /// Cancel all active checkpointer gates so in-flight saves complete but
    /// subsequent checkpoints are no-ops. Call during mob stop.
    async fn cancel_all_checkpointers(&self) {}

    /// Re-enable checkpointer gates after a prior cancel. Call during mob resume.
    async fn rearm_all_checkpointers(&self) {}
}

#[cfg(feature = "runtime-adapter")]
pub struct SessionBackend {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<MeerkatMachine>>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    ops_adapter: Arc<super::ops_adapter::MobOpsAdapter>,
    // Capability index for runtime bridge sidecars keyed by registered runtime
    // session identity. This map is never lifecycle truth; canonical
    // registration/attachment truth stays in MeerkatMachine.
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
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

#[cfg(feature = "runtime-adapter")]
impl SessionBackend {
    async fn replacement_runtime_session_state(
        existing: Arc<RuntimeSessionState>,
        preserve_missing_live_context: bool,
    ) -> Arc<RuntimeSessionState> {
        if preserve_missing_live_context {
            existing
        } else {
            existing.clear_queued_turns().await;
            Arc::new(RuntimeSessionState {
                queued_turns: Mutex::new(RuntimeSessionQueue::default()),
            })
        }
    }

    #[cfg(feature = "runtime-adapter")]
    pub(super) async fn restore_failed_resume_before_receipt(
        &self,
        session_id: &SessionId,
        restore_retired: bool,
    ) -> Result<(), MobError> {
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "resume rollback for '{session_id}' requires runtime authority"
            ))
        })?;
        if restore_retired {
            // A retired-session revival crosses two durable authorities before
            // executor attachment: Archived -> Active, then Retired -> Idle.
            // First remove the stale live projection: its agent snapshot was
            // materialized while Archived and is not the promoted document
            // authority. The archive protocol must read the durable Active
            // projection instead.
            match self.session_service.discard_live_session(session_id).await {
                Ok(()) | Err(SessionError::NotFound { .. }) => {}
                Err(error) => return Err(error.into()),
            }

            let retired_document = self
                .session_service
                .load_revivable_retired_session(session_id)
                .await?;
            let document_already_archived = retired_document.as_ref().is_some_and(|session| {
                session.lifecycle_terminal()
                    == Some(meerkat_core::session::SessionLifecycleTerminal::Archived)
            });

            if retired_document.is_some() && !document_already_archived {
                // Active+Retired is the crash-recoverable midpoint of revival.
                // Temporarily return the runtime to Idle so the document
                // archive authority cannot mistake Retired compatibility
                // evidence for an already-written Archived document.
                adapter.reset_runtime(session_id).await.map_err(|error| {
                    MobError::Internal(format!(
                        "failed to reopen revived session '{session_id}' for document rollback: {error}"
                    ))
                })?;
            }

            if !document_already_archived {
                match self
                    .session_service
                    .archive_with_mob_lifecycle_authority(session_id)
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

            if adapter.contains_session(session_id).await {
                self.unregister_runtime_session_binding(session_id).await?;
            }
            self.remove_runtime_session_state(session_id).await;
            return Ok(());
        }
        if adapter.contains_session(session_id).await {
            adapter
                .try_unregister_session(session_id)
                .await
                .map_err(|error| {
                    MobError::Internal(format!(
                        "failed to detach resumed session '{session_id}': {error}"
                    ))
                })?;
        }
        match self.session_service.discard_live_session(session_id).await {
            Ok(()) | Err(SessionError::NotFound { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        self.remove_runtime_session_state(session_id).await;
        Ok(())
    }

    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<MeerkatMachine>>,
        workgraph_service: Option<meerkat::WorkGraphService>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            workgraph_service,
            ops_adapter: Arc::new(super::ops_adapter::MobOpsAdapter::new()),
            runtime_sessions: Arc::new(RwLock::new(HashMap::new())),
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
        preserve_missing_live_context: bool,
    ) -> Result<Option<Arc<RuntimeSessionState>>, MobError> {
        let Some(adapter) = self.runtime_adapter.as_ref() else {
            return Ok(None);
        };
        #[cfg(target_arch = "wasm32")]
        if let Ok(runtime_sessions) = self.runtime_sessions.try_read()
            && let Some(existing) = runtime_sessions.get(session_id).cloned()
        {
            tracing::debug!(
                session_id = %session_id,
                "SessionBackend::runtime_session_state using cached wasm runtime session state"
            );
            return Ok(Some(existing));
        }
        if let Some(existing) = self.runtime_sessions.read().await.get(session_id).cloned() {
            if adapter
                .session_has_executor(session_id)
                .await
                .map_err(|error| MobError::Internal(error.to_string()))?
            {
                return Ok(Some(existing));
            }
            // Missing executor materialization does not invalidate session-
            // side turn ownership. Preserve that exact sidecar only for the
            // machine-authorized intent; ordinary replacement keeps the prior
            // clear-and-recreate behavior.
            let state =
                Self::replacement_runtime_session_state(existing, preserve_missing_live_context)
                    .await;
            let executor = Box::new(MobSessionRuntimeExecutor::new(
                self.session_service.clone(),
                Arc::clone(adapter),
                self.workgraph_service.clone(),
                session_id.clone(),
                state.clone(),
                Arc::clone(&self.runtime_sessions),
            ));
            // Runtime session registrations are capability bindings. Missing-
            // live revival reuses the exact context selected above; ordinary
            // replacement retains the prior fresh-sidecar behavior.
            #[cfg(target_arch = "wasm32")]
            {
                let adapter = Arc::clone(adapter);
                let session_id = session_id.clone();
                let (reply_tx, reply_rx) = oneshot::channel();
                tokio::spawn(async move {
                    let result = adapter
                        .ensure_session_with_executor(session_id, executor)
                        .await;
                    let _ = reply_tx.send(result);
                });
                reply_rx
                    .await
                    .map_err(|_| {
                        MobError::Internal("ensure session executor task was canceled".to_string())
                    })?
                    .map_err(|error| MobError::Internal(error.to_string()))?;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                adapter
                    .ensure_session_with_executor(session_id.clone(), executor)
                    .await
                    .map_err(|error| MobError::Internal(error.to_string()))?;
            }
            self.runtime_sessions
                .write()
                .await
                .insert(session_id.clone(), state.clone());
            return Ok(Some(state));
        }
        let state = Arc::new(RuntimeSessionState {
            queued_turns: Mutex::new(RuntimeSessionQueue::default()),
        });
        let executor = Box::new(MobSessionRuntimeExecutor::new(
            self.session_service.clone(),
            Arc::clone(adapter),
            self.workgraph_service.clone(),
            session_id.clone(),
            state.clone(),
            Arc::clone(&self.runtime_sessions),
        ));
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = Arc::clone(adapter);
            let session_id = session_id.clone();
            let (reply_tx, reply_rx) = oneshot::channel();
            tokio::spawn(async move {
                let result = adapter
                    .ensure_session_with_executor(session_id, executor)
                    .await;
                let _ = reply_tx.send(result);
            });
            reply_rx
                .await
                .map_err(|_| {
                    MobError::Internal("ensure session executor task was canceled".to_string())
                })?
                .map_err(|error| MobError::Internal(error.to_string()))?;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await
                .map_err(|error| MobError::Internal(error.to_string()))?;
        }
        self.runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), state.clone());
        Ok(Some(state))
    }

    async fn remove_runtime_session_state(&self, session_id: &SessionId) {
        let removed = self.runtime_sessions.write().await.remove(session_id);
        if let Some(state) = removed {
            state.clear_queued_turns().await;
        }
    }

    #[cfg(test)]
    pub(super) async fn has_runtime_session_sidecar_for_test(
        &self,
        session_id: &SessionId,
    ) -> bool {
        self.runtime_sessions.read().await.contains_key(session_id)
    }

    async fn unregister_runtime_session_binding(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        if let Some(adapter) = &self.runtime_adapter {
            #[cfg(target_arch = "wasm32")]
            if adapter
                .meerkat_machine_archive_snapshot(session_id)
                .await
                .is_some_and(|snapshot| {
                    matches!(
                        snapshot.control.phase,
                        meerkat_runtime::RuntimeState::Retired
                            | meerkat_runtime::RuntimeState::Stopped
                    ) && snapshot.queue.is_empty()
                        && snapshot.steer_queue.is_empty()
                })
            {
                tracing::warn!(
                    %session_id,
                    "skipping blocking runtime unregister for terminal storeless WASM session"
                );
                adapter.discard_terminal_storeless_session(session_id).await;
                self.remove_runtime_session_state(session_id).await;
                return Ok(());
            }

            // Unregister is idempotent and owns its complete two-phase drain.
            // The public call bounds each caller join and reports typed
            // UnregisterInProgress while the independently-owned saga keeps
            // running. Archive cleanup requires verified absence, so keep
            // joining that same saga rather than misclassifying its caller
            // grace as a terminal archive failure.
            loop {
                match adapter.unregister_session(session_id).await {
                    Ok(()) => break,
                    Err(meerkat_runtime::RuntimeDriverError::UnregisterInProgress {
                        runtime_id,
                    }) => {
                        tracing::debug!(
                            %session_id,
                            %runtime_id,
                            "mob archive cleanup is still joining the owned runtime unregister saga"
                        );
                    }
                    Err(error) => {
                        return Err(Self::runtime_archive_error(format!(
                            "failed to unregister runtime session during mob archive cleanup for {session_id}: {error}"
                        )));
                    }
                }
            }
            if adapter.contains_session(session_id).await {
                return Err(Self::runtime_archive_error(format!(
                    "runtime session remains registered after mob archive cleanup for {session_id}"
                )));
            }
            self.remove_runtime_session_state(session_id).await;
        }
        Ok(())
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_archive_error(message: impl Into<String>) -> SessionError {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(
            message.into(),
        ))
    }

    #[cfg(feature = "runtime-adapter")]
    fn retire_runtime_before_archive<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> ArchiveSessionFuture<'a> {
        Box::pin(async move {
            tracing::info!(
                session_id = %session_id,
                "SessionBackend::retire_runtime_before_archive start"
            );
            let Some(adapter) = &self.runtime_adapter else {
                return Ok(());
            };
            if !adapter.contains_session(session_id).await {
                return Ok(());
            }

            self.cancel_active_runtime_turn_before_retire(session_id)
                .await?;
            tracing::info!(
                session_id = %session_id,
                "SessionBackend::retire_runtime_before_archive active turn canceled"
            );

            let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
            let already_terminal = match adapter.meerkat_machine_archive_snapshot(session_id).await
            {
                Some(snapshot) => matches!(
                    snapshot.control.phase,
                    meerkat_runtime::RuntimeState::Retired | meerkat_runtime::RuntimeState::Stopped
                ),
                None => return Ok(()),
            };
            if !already_terminal {
                tracing::info!(
                    session_id = %session_id,
                    runtime_id = %runtime_id,
                    "SessionBackend::retire_runtime_before_archive calling runtime retire"
                );
                match adapter.retire_runtime_control_plane(&runtime_id).await {
                    Ok(_) => {}
                    Err(meerkat_runtime::RuntimeControlPlaneError::NotFound(_)) => return Ok(()),
                    Err(error) => {
                        return Err(Self::runtime_archive_error(format!(
                            "runtime retire before mob archive failed for {session_id}: {error}"
                        )));
                    }
                }
            }
            tracing::info!(
                session_id = %session_id,
                "SessionBackend::retire_runtime_before_archive runtime retired"
            );

            self.wait_for_runtime_retire_drain(session_id).await?;
            tracing::info!(
                session_id = %session_id,
                "SessionBackend::retire_runtime_before_archive drain complete"
            );
            Ok(())
        })
    }

    #[cfg(not(feature = "runtime-adapter"))]
    fn retire_runtime_before_archive<'a>(
        &'a self,
        _session_id: &'a SessionId,
    ) -> ArchiveSessionFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_run_bound(snapshot: &meerkat_runtime::MeerkatArchiveSnapshot) -> bool {
        snapshot.control.current_run_id.is_some()
    }

    #[cfg(feature = "runtime-adapter")]
    fn runtime_turn_cancellable(snapshot: &meerkat_runtime::MeerkatArchiveSnapshot) -> bool {
        snapshot.control.phase == meerkat_runtime::RuntimeState::Running
            && Self::runtime_run_bound(snapshot)
    }

    #[cfg(feature = "runtime-adapter")]
    async fn cancel_active_runtime_turn_before_retire(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        let Some(adapter) = &self.runtime_adapter else {
            return Ok(());
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut cancel_requested = false;

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

    #[cfg(feature = "runtime-adapter")]
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
    ) -> ArchiveSessionFuture<'a> {
        Box::pin(async move {
            tracing::info!(
                session_id = %session_id,
                "SessionBackend::archive_with_authority_then_unregister start"
            );
            self.retire_runtime_before_archive(session_id).await?;

            // Ownership read on the owning authority BEFORE archiving: a
            // member adopted from a host-owned session service
            // (`MemberLaunchMode::Resume` over a store the mob does not own,
            // e.g. an embedder's console sessions) legitimately has no
            // durable record here. For those, disposal means retiring the
            // runtime session and releasing the binding — the durable
            // session's lifecycle belongs to its host, and archiving through
            // the mob authority is not this mob's to perform. The
            // NotFound-with-registered-runtime escalation below stays fully
            // fail-closed for sessions the authority DOES own (a mid-archive
            // record loss is a genuine split-state, never tolerated).
            if !self
                .session_service
                .session_known_to_archive_authority(session_id)
                .await?
                && let Some(adapter) = &self.runtime_adapter
                && adapter.contains_session(session_id).await
            {
                tracing::info!(
                    session_id = %session_id,
                    "mob archive authority does not own this session's durable record; completing host-owned disposal (runtime retire + binding release)"
                );
                crate::runtime::session_service::retire_runtime_session_for_archive(
                    adapter.as_ref(),
                    session_id,
                )
                .await?;
                self.unregister_runtime_session_binding(session_id).await?;
                return Ok(());
            }

            tracing::info!(
                session_id = %session_id,
                "SessionBackend::archive_with_authority_then_unregister archiving session service"
            );
            match self
                .session_service
                .archive_with_mob_lifecycle_authority(session_id)
                .await
            {
                Ok(()) => {
                    tracing::info!(
                        session_id = %session_id,
                        "SessionBackend::archive_with_authority_then_unregister unregistering runtime binding"
                    );
                    self.unregister_runtime_session_binding(session_id).await?;
                    tracing::info!(
                        session_id = %session_id,
                        "SessionBackend::archive_with_authority_then_unregister complete"
                    );
                    Ok(())
                }
                Err(SessionError::NotFound { id }) => {
                    if let Some(adapter) = &self.runtime_adapter
                        && adapter.contains_session(session_id).await
                    {
                        // Ask 21d: the escalation below exists to protect a
                        // LIVE runtime the archive authority cannot see (a
                        // genuine split-state). A runtime in a TERMINAL
                        // lifecycle phase — the pre-archive retire has
                        // already run by this point — has no live
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
                            crate::runtime::session_service::retire_runtime_session_for_archive(
                                adapter.as_ref(),
                                session_id,
                            )
                            .await?;
                            self.unregister_runtime_session_binding(session_id).await?;
                            return Ok(());
                        }
                        return Err(SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(format!(
                                "mob archive authority returned NotFound for registered runtime session {session_id}"
                            )),
                        ));
                    }
                    self.unregister_runtime_session_binding(session_id).await?;
                    Err(SessionError::NotFound { id })
                }
                Err(error) => Err(error),
            }
        })
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
        let state = self.runtime_session_state(session_id, false).await?;
        let adapter_session_id = session_id.clone();
        let requested_input_id = input.id().clone();
        let mut context_input_id = requested_input_id.clone();
        let (queued_event_tx, deferred_delivery) =
            defer_turn_events_until_machine_completion(session_id, event_tx);

        // Queue only owner bridge context that cannot be reconstructed from
        // runtime primitives. Bind context by canonical input identity (not by
        // FIFO order) so runtime-owned contributing IDs remain the sole source
        // of semantic ordering.
        let queued_context = if let Some(ref state) = state {
            state
                .enqueue_turn_context(requested_input_id.clone(), queued_event_tx)
                .await
        } else {
            false
        };

        let (outcome, handle) = match adapter
            .accept_input_with_completion(&adapter_session_id, input)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                if let Some(state) = state.as_ref()
                    && queued_context
                {
                    let _ = state.discard_turn_context(&requested_input_id).await;
                }
                let error = err.to_string();
                if let Some(delivery) = deferred_delivery {
                    delivery.release();
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

        if let Some(state) = state.as_ref()
            && queued_context
        {
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
                let rekeyed = state
                    .rekey_turn_context(&requested_input_id, input_id.clone())
                    .await;
                if rekeyed {
                    context_input_id = input_id.clone();
                }
            }
        }

        // Terminal dedup: input already processed — idempotent success
        let Some(handle) = handle else {
            if let Some(state) = state.as_ref()
                && queued_context
            {
                let _ = state.discard_turn_context(&context_input_id).await;
            }
            if let Some(delivery) = deferred_delivery {
                delivery.release();
            }
            return Ok(());
        };

        let completion = handle.wait().await;
        if let Some(state) = state.as_ref()
            && queued_context
        {
            let _ = state.discard_turn_context(&context_input_id).await;
        }
        if let Some(delivery) = deferred_delivery {
            delivery.release();
        }

        runtime_completion_to_mob_result(session_id, completion)
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
    ) -> Result<(), MobError> {
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "runtime-backed turn requested without runtime adapter: {session_id}"
            ))
        })?;
        let state = self.runtime_session_state(session_id, false).await?;
        let adapter_session_id = session_id.clone();
        let requested_input_id = input.id().clone();
        let (queued_event_tx, deferred_delivery) =
            defer_turn_events_until_machine_completion(session_id, event_tx);

        let queued_context = if let Some(ref state) = state {
            state
                .enqueue_turn_context(requested_input_id.clone(), queued_event_tx)
                .await
        } else {
            false
        };

        #[cfg(target_arch = "wasm32")]
        {
            // Browser WASM runs the mob actor and runtime adapter on the same
            // single-thread executor. Polling this admission while the actor
            // command is still active starves the spawned runtime task, so the
            // standalone browser substrate schedules admission across the
            // actor boundary. Native runtime-backed paths keep the stricter
            // synchronous admission result below.
            let input = Box::new(input);
            let adapter = Arc::clone(adapter);
            let task_session_id = adapter_session_id.clone();
            let task_input_id = requested_input_id.clone();
            tokio::spawn(async move {
                if let Err(error) = MeerkatMachine::accept_boxed_input_with_completion(
                    adapter.as_ref(),
                    &task_session_id,
                    input,
                    task_input_id.clone(),
                )
                .await
                {
                    tracing::warn!(
                        session_id = %task_session_id,
                        input_id = %task_input_id,
                        %error,
                        "background WASM runtime input admission failed"
                    );
                }
            });
            if let Some(state) = state.as_ref()
                && queued_context
            {
                let _ = state.discard_turn_context(&requested_input_id).await;
            }
            if let Some(delivery) = deferred_delivery {
                delivery.release();
            }
            Ok(())
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut context_input_id = requested_input_id.clone();
            let accept_result = adapter
                .accept_input_with_completion(&adapter_session_id, input)
                .await;

            let (outcome, handle) = match accept_result {
                Ok(result) => result,
                Err(err) => {
                    if let Some(state) = state.as_ref()
                        && queued_context
                    {
                        let _ = state.discard_turn_context(&requested_input_id).await;
                    }
                    let error = err.to_string();
                    if let Some(delivery) = deferred_delivery {
                        delivery.release();
                    }
                    return Err(MobError::Internal(error));
                }
            };

            if let Some(state) = state.as_ref()
                && queued_context
            {
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
                    let rekeyed = state
                        .rekey_turn_context(&requested_input_id, input_id.clone())
                        .await;
                    if rekeyed {
                        context_input_id = input_id.clone();
                    }
                }
            }

            let Some(handle) = handle else {
                if let Some(state) = state.as_ref()
                    && queued_context
                {
                    let _ = state.discard_turn_context(&context_input_id).await;
                }
                if let Some(delivery) = deferred_delivery {
                    delivery.release();
                }
                return Ok(());
            };

            if let Some(state) = state
                && queued_context
            {
                tokio::spawn(async move {
                    let _completion = handle.wait().await;
                    let _ = state.discard_turn_context(&context_input_id).await;
                    if let Some(delivery) = deferred_delivery {
                        delivery.release();
                    }
                });
            } else if let Some(delivery) = deferred_delivery {
                tokio::spawn(async move {
                    let _completion = handle.wait().await;
                    delivery.release();
                });
            }

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

fn session_error_means_session_identity_already_active(
    error: &SessionError,
    bridge_session_id: &SessionId,
) -> bool {
    matches!(
        error,
        SessionError::Agent(
            meerkat_core::error::AgentError::SessionIdentityInUse(session_id)
        ) if session_id == bridge_session_id
    )
}

#[cfg(feature = "runtime-adapter")]
pub(super) struct RuntimeSessionState {
    // Transport-only owner context keyed by canonical runtime input identity.
    // Never used as lifecycle/ordering truth; ordering is runtime-owned via
    // contributing_input_ids and input lifecycle state.
    queued_turns: Mutex<RuntimeSessionQueue>,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Default)]
struct RuntimeSessionQueue {
    // Event delivery transport handles for runtime-backed turn dispatch.
    entries: HashMap<InputId, QueuedTurnContext>,
}

#[cfg(feature = "runtime-adapter")]
struct QueuedTurnContext {
    event_tx: TurnEventTx,
}

#[cfg(feature = "runtime-adapter")]
impl RuntimeSessionState {
    #[cfg(test)]
    pub(super) fn empty_for_test() -> Self {
        Self {
            queued_turns: Mutex::new(RuntimeSessionQueue::default()),
        }
    }

    async fn enqueue_turn_context(&self, input_id: InputId, event_tx: Option<TurnEventTx>) -> bool {
        let Some(event_tx) = event_tx else {
            return false;
        };
        let mut queued_turns = self.queued_turns.lock().await;
        queued_turns
            .entries
            .insert(input_id, QueuedTurnContext { event_tx });
        true
    }

    async fn take_turn_context_for_inputs(
        &self,
        contributing_input_ids: &[InputId],
    ) -> Option<QueuedTurnContext> {
        let mut queued_turns = self.queued_turns.lock().await;
        let mut selected: Option<QueuedTurnContext> = None;
        for input_id in contributing_input_ids {
            if let Some(context) = queued_turns.entries.remove(input_id) {
                // A runtime primitive may contribute multiple input IDs
                // (staged/apply-boundary cases). Drain every matching key so the
                // bridge map never accumulates stale side entries; prefer the
                // most-recent matching contributor in canonical order.
                selected = Some(context);
            }
        }
        selected
    }

    async fn rekey_turn_context(&self, from_input_id: &InputId, to_input_id: InputId) -> bool {
        if from_input_id == &to_input_id {
            return true;
        }
        let mut queued_turns = self.queued_turns.lock().await;
        if queued_turns.entries.contains_key(&to_input_id) {
            // Keep the canonical destination binding and drop the source alias.
            queued_turns.entries.remove(from_input_id);
            return true;
        }
        let Some(context) = queued_turns.entries.remove(from_input_id) else {
            return false;
        };
        queued_turns.entries.insert(to_input_id, context);
        true
    }

    async fn discard_turn_context(&self, input_id: &InputId) -> bool {
        let mut queued_turns = self.queued_turns.lock().await;
        queued_turns.entries.remove(input_id).is_some()
    }

    async fn clear_queued_turns(&self) {
        self.queued_turns.lock().await.entries.clear();
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        MultiBackendProvisioner, defer_turn_events_until_machine_completion,
        runtime_completion_to_mob_result, session_turn_error_to_mob_error,
    };
    use crate::error::MobError;
    use meerkat_core::service::SessionError;
    use meerkat_core::types::SessionId;
    use serde_json::json;

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
        deferred_delivery.release();
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
    async fn missing_live_replacement_preserves_exact_queued_turn_sidecar() {
        let state = std::sync::Arc::new(super::RuntimeSessionState::empty_for_test());
        let input_id = meerkat_core::lifecycle::InputId::new();
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(1);
        assert!(
            state
                .enqueue_turn_context(input_id.clone(), Some(event_tx))
                .await
        );

        let replacement = super::SessionBackend::replacement_runtime_session_state(
            std::sync::Arc::clone(&state),
            true,
        )
        .await;

        assert!(
            std::sync::Arc::ptr_eq(&replacement, &state),
            "MissingLive replacement must retain the exact sidecar owner"
        );
        assert!(
            replacement
                .take_turn_context_for_inputs(&[input_id])
                .await
                .is_some(),
            "queued TurnEventTx context must survive executor replacement"
        );
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
    async fn cancel_after_boundary(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.session_service
            .cancel_after_boundary_with_machine_authority(
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

    async fn active_turn_boundary_available(&self) -> Result<bool, CoreExecutorError> {
        Ok(self
            .session_service
            .active_turn_system_context_boundary_available(&self.bridge_session_id)
            .await
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))?
            .unwrap_or(false))
    }

    async fn stage_system_context_at_boundary(
        &self,
        expected_run_id: &CoreRunId,
        appends: Vec<PendingSystemContextAppend>,
    ) -> Result<meerkat_core::lifecycle::CoreBoundaryStageOutput, CoreExecutorError> {
        let session_snapshot = self
            .session_service
            .stage_runtime_system_context_for_active_turn(
                &self.bridge_session_id,
                expected_run_id,
                appends,
            )
            .await
            .map_err(|err| CoreExecutorError::apply_failed_runtime_context(err.to_string()))?;
        Ok(meerkat_core::lifecycle::CoreBoundaryStageOutput::new(
            session_snapshot,
        ))
    }

    async fn discard_staged_system_context_at_boundary(
        &self,
        expected_run_id: &CoreRunId,
        idempotency_keys: Vec<String>,
    ) -> Result<(), CoreExecutorError> {
        self.session_service
            .discard_runtime_system_context_for_active_turn(
                &self.bridge_session_id,
                expected_run_id,
                idempotency_keys,
            )
            .await
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }
}

#[cfg(feature = "runtime-adapter")]
struct MobSessionServiceInterruptHandle {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Arc<MeerkatMachine>,
    bridge_session_id: SessionId,
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
                unreachable!("context-only apply without turn only matches staged primitives");
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
            event_tx: queued_context.map(|context| context.event_tx),
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
            .checkpoint_committed_runtime_session_snapshot(
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

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.session_service
            .cancel_after_boundary_with_machine_authority(
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
        tracing::debug!(
            bridge_session_id = %self.bridge_session_id,
            "mob runtime executor received machine-authorized external cleanup"
        );
        // Runtime removal belongs to the machine-owned unregister saga. This
        // hook owns only provisioner/session material, so it cannot recursively
        // join the coordinator that is currently invoking it.
        let discard_result = self
            .session_service
            .discard_live_session(&self.bridge_session_id)
            .await;
        match discard_result {
            Ok(()) | Err(SessionError::NotFound { .. }) => {}
            Err(err) => {
                return Err(CoreExecutorError::control_failed_runtime(err.to_string()));
            }
        }
        let removed = {
            let mut runtime_sessions = self.runtime_sessions.write().await;
            let should_remove = runtime_sessions
                .get(&self.bridge_session_id)
                .is_some_and(|state| Arc::ptr_eq(state, &self.state));
            if should_remove {
                runtime_sessions.remove(&self.bridge_session_id)
            } else {
                None
            }
        };
        if let Some(state) = removed {
            state.clear_queued_turns().await;
        } else {
            self.state.clear_queued_turns().await;
        }
        Ok(())
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobProvisioner for SessionBackend {
    async fn provision_member(
        &self,
        mut req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError> {
        let mut session_origin = req.session_origin;
        let missing_live_materialization =
            req.runtime_revival_intent == RuntimeRevivalIntent::MissingLiveMaterialization;
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
        let mut reviving_retired_session = false;
        let pre_registered_bridge_session_id = if let Some(adapter) = &self.runtime_adapter {
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
            // MobMachine::Spawn routes the authoritative member runtime id
            // and fence token after the bridge session exists. This call only
            // needs the session-local handle bundle for AgentFactory.
            tracing::debug!(
                bridge_session_id = %member_bridge_session_id,
                "SessionBackend::provision_member preparing local session bindings"
            );
            #[cfg(target_arch = "wasm32")]
            let bindings = {
                let adapter = Arc::clone(adapter);
                let bridge_session_id = member_bridge_session_id.clone();
                let (reply_tx, reply_rx) = oneshot::channel();
                tokio::spawn(async move {
                    let result = adapter
                        .prepare_local_session_bindings(bridge_session_id)
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
            let bindings = adapter
                .prepare_local_session_bindings(member_bridge_session_id.clone())
                .await
                .map_err(|e| {
                    MobError::Internal(format!("prepare local session bindings failed: {e}"))
                })?;
            tracing::debug!(
                bridge_session_id = %member_bridge_session_id,
                "SessionBackend::provision_member prepared local session bindings"
            );
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
            }
            if missing_live_materialization && !reviving_retired_session {
                let observed_runtime_state = adapter
                    .runtime_state(&member_bridge_session_id)
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to inspect missing-live member runtime '{member_bridge_session_id}': {error}"
                        ))
                    })?;
                let has_live_executor = adapter
                    .session_has_executor(&member_bridge_session_id)
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to inspect missing-live member executor '{member_bridge_session_id}': {error}"
                        ))
                    })?;
                // Only exact Idle/Attached absence is a missing-owner proof.
                // Stopped continues through ordinary stopped-session revival,
                // while a genuinely live executor remains the exact owner and
                // is reused by the rebuilt session.
                if matches!(
                    observed_runtime_state,
                    meerkat_runtime::RuntimeState::Idle | meerkat_runtime::RuntimeState::Attached
                ) && !has_live_executor
                {
                    #[cfg(target_arch = "wasm32")]
                    let redrive_result = {
                        let adapter = Arc::clone(adapter);
                        let bridge_session_id = member_bridge_session_id.clone();
                        let authority = adapter.session_control_authority();
                        let (reply_tx, reply_rx) = oneshot::channel();
                        tokio::spawn(async move {
                            let result = adapter
                                .redrive_missing_executor_for_revival(&bridge_session_id, authority)
                                .await;
                            let _ = reply_tx.send(result);
                        });
                        reply_rx.await.map_err(|_| {
                            MobError::Internal(
                                "missing-live runtime re-drive task was canceled".to_string(),
                            )
                        })?
                    };
                    #[cfg(not(target_arch = "wasm32"))]
                    let redrive_result = adapter
                        .redrive_missing_executor_for_revival(
                            &member_bridge_session_id,
                            adapter.session_control_authority(),
                        )
                        .await;
                    match redrive_result {
                        Ok(()) => {}
                        Err(meerkat_runtime::RuntimeDriverError::NotReady {
                            state: meerkat_runtime::RuntimeState::Stopped,
                        }) => {
                            tracing::debug!(
                                bridge_session_id = %member_bridge_session_id,
                                "missing-live revival raced canonical runtime stop; ordinary stopped-session revival now owns readmission"
                            );
                        }
                        Err(error) => {
                            if adapter
                                .session_has_executor(&member_bridge_session_id)
                                .await
                                .is_ok_and(|present| present)
                            {
                                tracing::debug!(
                                    bridge_session_id = %member_bridge_session_id,
                                    "missing-live revival observed a concurrent executor materialization"
                                );
                            } else {
                                return Err(MobError::Internal(format!(
                                    "failed to re-drive missing-live member runtime '{member_bridge_session_id}': {error}"
                                )));
                            }
                        }
                    }
                }
            }
            if let Some(ref mut build) = req.create_session.build {
                build.runtime_build_mode =
                    meerkat_core::runtime_epoch::RuntimeBuildMode::SessionOwned(bindings);
            }
            prepared_ops_binding = Some((member_bridge_session_id.clone(), ops_registry));
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
        tracing::debug!("SessionBackend::provision_member creating bridge session");
        let create_result = if reviving_retired_session {
            let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
                MobError::Internal(
                    "retired durable session revival requires runtime authority".into(),
                )
            })?;
            self.session_service
                .create_session_with_machine_archived_resume_authority(
                    req.create_session,
                    adapter.session_control_authority(),
                )
                .await
        } else {
            self.session_service
                .create_session(req.create_session)
                .await
        };
        let created = match create_result {
            Ok(created) => created,
            Err(e) => {
                // Rollback: unregister the pre-registered session on failure
                if let (Some(adapter), Some(pre_id)) =
                    (&self.runtime_adapter, &pre_registered_bridge_session_id)
                    && !session_error_means_session_identity_already_active(&e, pre_id)
                {
                    if let Err(cleanup_error) = adapter.unregister_session(pre_id).await {
                        return Err(MobError::Internal(format!(
                            "{e}; additionally failed to unregister pre-registered runtime session {pre_id}: {cleanup_error}"
                        )));
                    }
                }
                return Err(e.into());
            }
        };
        tracing::debug!(
            bridge_session_id = %created.session_id,
            "SessionBackend::provision_member created session service session"
        );
        let created_bridge_session_id = created.session_id.clone();
        let rollback_origin = session_origin;
        let finalize_result = async {
        if let Some(admitted_bridge_session_id) = admitted_bridge_session_id.as_ref()
            && admitted_bridge_session_id != &created_bridge_session_id
        {
            if let Err(error) = self
                .archive_with_authority_then_unregister(&created_bridge_session_id)
                .await
                && !matches!(error, SessionError::NotFound { .. })
            {
                return Err(MobError::Internal(format!(
                    "session service returned bridge session '{created_bridge_session_id}' for admitted mob spawn session '{admitted_bridge_session_id}', and cleanup archive failed: {error}"
                )));
            }
            self.unregister_runtime_session_binding(admitted_bridge_session_id)
                .await?;
            return Err(MobError::Internal(format!(
                "session service returned bridge session '{created_bridge_session_id}' for admitted mob spawn session '{admitted_bridge_session_id}'"
            )));
        }
        // If no admission id was supplied, clean up stale local pre-registration
        // defensively. Normal mob spawn paths now admit a concrete session id
        // before provisioning starts.
        if let (Some(adapter), Some(pre_id)) =
            (&self.runtime_adapter, &pre_registered_bridge_session_id)
        {
            if *pre_id != created_bridge_session_id {
                tracing::debug!(
                    pre_registered = %pre_id,
                    actual_bridge_session_id = %created_bridge_session_id,
                    "mob provisioner: session service returned different ID; reconciling runtime registration"
                );
                adapter.unregister_session(pre_id).await.map_err(|error| {
                    MobError::Internal(format!(
                        "failed to unregister stale pre-registered runtime session {pre_id}: {error}"
                    ))
                })?;
            }
            if reviving_retired_session {
                // Explicit revival owns the only path across both absorbing
                // lifecycle boundaries. Prove and promote the archived
                // document while the runtime is still durably Retired, then
                // reset Retired -> Idle before generic executor attachment.
                // Retired must remain non-registrable on every ordinary
                // ensure-session path.
                self.session_service
                    .promote_revivable_retired_session(
                        &created_bridge_session_id,
                        adapter.session_control_authority(),
                    )
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to promote revived durable session document '{created_bridge_session_id}': {error}"
                        ))
                    })?;
                adapter
                    .reset_runtime(&created_bridge_session_id)
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
                "SessionBackend::provision_member checking runtime session state"
            );
            self.runtime_session_state(
                &created_bridge_session_id,
                missing_live_materialization,
            )
                .await?;
            tracing::debug!(
                bridge_session_id = %created_bridge_session_id,
                "SessionBackend::provision_member checked runtime session state"
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
                self.ops_adapter.bind_session_registry(
                    created_bridge_session_id.clone(),
                    owner_bridge_session_id,
                    registry,
                );
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
                self.ops_adapter.bind_session_registry(
                    created_bridge_session_id.clone(),
                    generated_owner_session_id,
                    registry,
                );
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
        let operation_id = self
            .ops_adapter
            .mark_member_provisioned(&created_bridge_session_id, &req.peer_name)
            .await?;
        tracing::debug!(
            bridge_session_id = %created_bridge_session_id,
            operation_id = %operation_id,
            "SessionBackend::provision_member marked member provisioned"
        );
        tracing::debug!(
            bridge_session_id = %created_bridge_session_id,
            "SessionBackend::provision_member created bridge session"
        );
        Ok(MemberSpawnReceipt {
            member_ref: MemberRef::from_bridge_session_id(created_bridge_session_id),
            operation_id,
            session_origin,
        })
        }
        .await;
        if let Err(error) = finalize_result {
            if rollback_origin != ProvisionSessionOrigin::Fresh
                && let Err(cleanup_error) = self
                    .restore_failed_resume_before_receipt(
                        &created.session_id,
                        rollback_origin == ProvisionSessionOrigin::RevivedRetired
                            || reviving_retired_session,
                    )
                    .await
            {
                return Err(MobError::Internal(format!(
                    "resume provision failed: {error}; durable rollback failed: {cleanup_error}"
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
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
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
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            }
            Some((_status, true)) => {
                match self
                    .archive_with_authority_then_unregister(&bridge_session_id)
                    .await
                {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            }
            Some((_status, false)) => self.retire_member(member_ref).await,
        }
    }

    async fn restore_resumed_member(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        original_origin: ProvisionSessionOrigin,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "restore failed resume for")?;
        if original_origin == ProvisionSessionOrigin::RevivedRetired {
            return match self
                .archive_with_authority_then_unregister(&session_id)
                .await
            {
                Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                Err(error) => Err(error.into()),
            };
        }
        let Some(adapter) = &self.runtime_adapter else {
            return Err(MobError::Internal(format!(
                "cannot restore resumed session '{session_id}' without runtime authority"
            )));
        };

        if adapter.contains_session(&session_id).await {
            adapter
                .try_unregister_session(&session_id)
                .await
                .map_err(|error| {
                    MobError::Internal(format!(
                        "failed to return resumed session '{session_id}' to durable idle: {error}"
                    ))
                })?;
        }
        match self.session_service.discard_live_session(&session_id).await {
            Ok(()) | Err(SessionError::NotFound { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        self.remove_runtime_session_state(&session_id).await;

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

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        tracing::debug!(
            member_ref = ?member_ref,
            "SessionBackend::retire_member start"
        );
        let session_id = Self::require_session(member_ref, "retire")?;
        self.archive_with_authority_then_unregister(&session_id)
            .await?;
        self.ops_adapter.mark_member_retired(member_ref).await?;
        Ok(())
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "interrupt")?;
        if let Some(adapter) = &self.runtime_adapter {
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
                            self.remove_runtime_session_state(&session_id).await;
                        }
                        Err(MobError::Internal(format!(
                            "runtime-backed interrupt must resolve through MeerkatMachine for '{session_id}': {error}"
                        )))
                    }
                };
            }

            // Runtime-backed members must be interrupted through runtime
            // adapter registration truth, not direct session-service fallback.
            self.remove_runtime_session_state(&session_id).await;
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

            self.remove_runtime_session_state(&session_id).await;
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
                .admit_runtime_input(&session_id, input, req.event_tx)
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
                .admit_runtime_input(&session_id, input, req.event_tx)
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
                .admit_runtime_input(&session_id, input, req.event_tx)
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

    async fn ensure_runtime_session_state(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let bridge_session_id = Self::require_session(member_ref, "ensure runtime session for")?;
        self.runtime_session_state(&bridge_session_id, false)
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
        );
        Ok(())
    }

    async fn cancel_all_checkpointers(&self) {
        self.session_service.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.session_service.rearm_all_checkpointers().await;
    }
}

pub struct ExternalBackend {
    _session_service: Arc<dyn MobSessionService>,
}

impl ExternalBackend {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        _config: ExternalBackendConfig,
    ) -> Self {
        Self {
            _session_service: session_service,
        }
    }
}

#[cfg(feature = "runtime-adapter")]
pub struct MultiBackendProvisioner {
    session: SessionBackend,
    external: Option<ExternalBackend>,
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
        let external = external.map(|cfg| ExternalBackend::new(session_service, cfg));
        Self {
            session,
            external,
            supervisor_bridge,
            binding_persistence: None,
        }
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
        let install = self.supervisor_bridge.trust_recipient(peer).await?;
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
                    Self::bridge_bootstrap_token_from_binding(address, bootstrap_token)?;
                let expected_address = super::bridge_protocol::canonicalize_bridge_address(address);
                let expected_peer =
                    Self::peer_only_spec_from_parts(peer_id, &expected_address, pubkey)?;
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
                        return Err(MobError::WiringError(format!(
                            "AuthorizeSupervisor rejection for '{peer_id}' could not roll back rejected recipient trust: {untrust_error}"
                        )));
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
                if let Some((current, pending)) = self.pending_supervisor_rotation().await? {
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
                // Re-bind the same peer (the bind re-establishes its own
                // recipient trust). If the bind fails, the recipient was never
                // confirmed and its trust must not survive — roll it back.
                let bind: super::bridge_protocol::BridgeBindResponse = match self
                    .bind_peer_only_member(peer, peer_id, address, bootstrap_token)
                    .await
                {
                    Ok(bind) => bind,
                    Err(bind_error) => {
                        return Err(self
                            .rollback_supervisor_recipient_trust(peer, install, bind_error)
                            .await);
                    }
                };
                let returned_address =
                    super::bridge_protocol::canonicalize_bridge_address(&bind.address);
                if bind.peer_id != peer_id || returned_address != expected_address {
                    return Err(MobError::WiringError(format!(
                        "peer-only bind response changed authorized endpoint for '{peer_id}'"
                    )));
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
            return MobError::WiringError(format!(
                "supervisor authorization failed ({original_error}); additionally failed to roll back installed recipient trust: {untrust_error}"
            ));
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
        // Trust is installed ahead of the routed send (comms admission).
        // Every non-confirmed outcome — send failure, terminal rejection,
        // decode failure — rolls trust newly installed by THIS call back
        // fail-closed, mirroring the actor's `ensure_supervisor_authorized`;
        // pre-existing confirmed trust is left in place. The MobMachine
        // `pending_recipient_trust` obligation for this window is owned by
        // the actor around external provisioning (the provisioner cannot
        // reach MobMachine authority).
        let install = self.supervisor_bridge.trust_recipient(peer).await?;
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
            Ok(payload) => Ok(payload),
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
    ) -> Result<super::bridge_protocol::BridgeBindResponse, MobError> {
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
        self.send_bridge_command_typed(peer, &command, Duration::from_secs(60))
            .await
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
            "ExternalBackend::external_member_ref start"
        );
        let _external = self
            .external
            .as_ref()
            .ok_or_else(|| MobError::WiringError("external backend is not configured".into()))?;
        tracing::debug!(
            peer_id = %real_peer_id,
            address = %real_address,
            "ExternalBackend::external_member_ref success (real identity)"
        );
        let peer =
            Self::validated_external_peer_spec(&peer_name, &real_peer_id, &real_address, pubkey)?;
        let bind_response = self
            .bind_peer_only_member(
                &peer,
                &real_peer_id,
                &real_address,
                Some(effective_bootstrap_token.as_str()),
            )
            .await?;
        let canonical_address =
            super::bridge_protocol::canonicalize_bridge_address(&bind_response.address);
        let validated_bind_response = Self::validated_external_peer_spec(
            &peer_name,
            &bind_response.peer_id,
            &canonical_address,
            pubkey,
        )?;
        let operation_source = OperationSource::backend_peer(
            validated_bind_response.peer_id,
            validated_bind_response.address.clone(),
        );
        let member_ref = MemberRef::BackendPeer {
            peer_id: bind_response.peer_id,
            address: canonical_address,
            pubkey,
            bootstrap_token: Some(effective_bootstrap_token),
            session_id: None,
        };
        self.session.ops_adapter.bind_member_registry(
            &member_ref,
            owner_bridge_session_id,
            ops_registry,
            peer_name.clone(),
            operation_source,
        )?;
        let operation_id = self
            .session
            .ops_adapter
            .mark_member_provisioned_for_member(&member_ref, &peer_name)
            .await?;
        Ok(MemberSpawnReceipt {
            member_ref,
            operation_id,
            session_origin: ProvisionSessionOrigin::Fresh,
        })
    }
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobProvisioner for MultiBackendProvisioner {
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
        }
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

    async fn restore_resumed_member(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        original_origin: ProvisionSessionOrigin,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None, ..
            } => Err(MobError::Internal(
                "peer-only provision cannot be classified as a resumed durable session".into(),
            )),
            _ => {
                self.session
                    .restore_resumed_member(member_ref, operation_id, original_origin)
                    .await
            }
        }
    }

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
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
                    .await
            }
            _ => self.session.retire_member(member_ref).await,
        }
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
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
                    // No MobMachine authority is reachable on the interrupt
                    // path, so any rejection is bubbled up uniformly without
                    // re-deriving the MobMachine-owned recovery verdict.
                    return Err(MobError::BridgeCommandRejected {
                        cause: observation.rejection_cause,
                        reason: "peer-only interrupt was rejected by the remote member".to_string(),
                    });
                }
                let peer = authorization.peer;
                let payload = self.bridge_supervisor_payload_for_recipient(&peer).await?;
                let command = super::bridge_protocol::BridgeCommand::InterruptMember(payload);
                let _ack: super::bridge_protocol::BridgeAck = self
                    .send_bridge_command_typed(&peer, &command, Duration::from_secs(5))
                    .await?;
                Ok(())
            }
            _ => self.session.interrupt_member(member_ref).await,
        }
    }

    async fn hard_cancel_member(
        &self,
        member_ref: &MemberRef,
        reason: &str,
    ) -> Result<(), MobError> {
        match member_ref {
            MemberRef::BackendPeer {
                session_id: None,
                ..
            } => Err(MobError::Internal(
                "peer-only external members cannot be hard-cancelled over supervisor bridge; use cooperative interrupt_member"
                    .to_string(),
            )),
            _ => self.session.hard_cancel_member(member_ref, reason).await,
        }
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
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
                    super::bridge_protocol::BridgeDeliveryPayload {
                        supervisor: sup_spec.into(),
                        epoch: authority.epoch,
                        protocol_version: authority.protocol_version,
                        input_id: meerkat_core::time_compat::new_uuid_v7().to_string(),
                        content: req.prompt.clone(),
                        handling_mode: req.runtime.handling_mode,
                        objective_id: req
                            .runtime
                            .turn_metadata
                            .as_ref()
                            .and_then(|metadata| metadata.transcript_identity.objective_id),
                        // Rides the delivery payload; the receiving runtime
                        // lowers it as InjectedContext-role appends before
                        // the peer work append (empty is omitted on the
                        // wire, keeping pre-field byte compatibility).
                        injected_context: req.injected_context.clone(),
                    },
                );
                let response: super::bridge_protocol::BridgeDeliveryResponse = self
                    .send_bridge_command_typed(&peer, &command, Duration::from_secs(5))
                    .await?;
                match response.outcome {
                    super::bridge_protocol::BridgeDeliveryOutcome::Accepted
                    | super::bridge_protocol::BridgeDeliveryOutcome::Deduplicated { .. } => {}
                    super::bridge_protocol::BridgeDeliveryOutcome::Rejected { cause, reason } => {
                        return Err(MobError::BridgeDeliveryRejected { cause, reason });
                    }
                }
                self.session
                    .ops_adapter
                    .report_member_progress(member_ref, "turn dispatched")
                    .await?;
                Ok(())
            }
            _ => self.session.start_turn(member_ref, req).await,
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
    use super::MultiBackendProvisioner;
    use crate::MobError;
    use crate::runtime::bridge_protocol::{
        BridgeRejectionCause, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        decode_legacy_v1_raw_string_rejection,
    };
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
}
