//! Facade member-host live adapters (phase 6b, DEC-P6B-L10/L11).
//!
//! [`ServiceMemberLiveHost`] is the owned `meerkat_runtime::member_live::
//! MemberLiveHost` implementation the mob host daemon (and any live-capable
//! runtime composition) installs on `MeerkatMachine::set_member_live_host`.
//! It builds the borrowing [`LiveOrchestrator`] per trait call — the SAME
//! extracted pipeline `rkat-rpc`'s `live/*` handlers run — so a
//! bridge-delivered open executes the identical S1-S12 sequence, including
//! the fail-closed open-failure cleanup and the mob-owned peer-ingress
//! skip. Bootstrap URLs mint against the composition's advertised base URL,
//! so cross-host correctness is by-construction (DL6, seam S5).
//!
//! [`ServiceLiveToolDispatcher`] closes the §16.2 tool-parity gap: live
//! tool calls raised mid-turn on a member-host channel dispatch into the
//! owning session's dispatcher through the canonical
//! `dispatch_external_tool_call_with_timeout_policy` service seam — the
//! same seam `rkat-rpc`'s `RuntimeLiveToolDispatcher` shims.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_contracts::wire::supervisor_bridge::{BridgeLiveControlOutcome, BridgeLiveControlVerb};
use meerkat_contracts::{LiveCloseStatus, LiveOpenResult, LiveOpenTransport, RealtimeTurningMode};
#[cfg(feature = "experimental-gpt-live")]
use meerkat_contracts::{WireLiveExecutionIdentityOverrideV1, WireLiveExecutionIdentityVersion};
use meerkat_core::connection::RealmId;
#[cfg(feature = "experimental-gpt-live")]
use meerkat_core::live_adapter::LiveInputChunk;
use meerkat_core::types::SessionId;
use meerkat_live::{LiveAdapterHost, LiveChannelId, LiveWsState};
#[cfg(feature = "live-webrtc")]
use meerkat_live::{
    LiveWebrtcAdmittedOffer, LiveWebrtcAnswerTransport, LiveWebrtcBindingRequest, LiveWebrtcError,
    ProviderWebrtcPendingBoundReadySeal,
};
use meerkat_llm_core::realtime_session::RealtimeSessionFactory;
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::member_live::{
    MEMBER_LIVE_OPEN_CEILING, MemberLiveError, MemberLiveHost, MemberLiveStatus,
};

#[cfg(feature = "experimental-gpt-live")]
use crate::experimental_gpt_live::{
    ExperimentalLiveOpenAuthorityError, ExperimentalLivePhysicalClose,
};
use crate::service_factory::FactoryAgentBuilder;
use crate::session_runtime::admission::StagedCapacityAdmissions;
use crate::session_runtime::errors::{LiveChannelVerbError, LiveIngressError, LiveOpenError};
#[cfg(feature = "experimental-gpt-live")]
use crate::session_runtime::live_orchestration::ExperimentalLiveChannelOpenError;
#[cfg(feature = "experimental-gpt-live")]
use crate::session_runtime::live_orchestration::ExperimentalLivePendingChannel;
use crate::session_runtime::live_orchestration::{
    LiveOrchestrator, LiveSeedWindow, LiveSessionIngressReconciler, LiveTransportContext,
    RealtimeSessionOpenProjection, RealtimeSessionOpenProjectionError,
};
use crate::session_runtime::runtime_state::ArchiveRuntimeCleanup;
use crate::{PersistentSessionService, SessionAgentBuilder, StagedSessionRegistry};

/// Provider-neutral client bootstrap for a fresh channel required after an
/// ambiguous experimental-live delivery. The variant is the only exposed
/// reason; result content, digests, provider identifiers, and internal
/// authority remain sealed server-side.
#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExperimentalLiveReplacementRequired {
    CanonicalContext {
        open: LiveOpenResult,
        canonical_seed_cursor: u64,
    },
    DelegationResult {
        open: LiveOpenResult,
        canonical_seed_cursor: u64,
    },
}

/// Public-safe projection of one machine-minted playback owner readiness.
#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
#[derive(Clone)]
pub struct ExperimentalLivePlaybackOwnerReadiness {
    channel_id: LiveChannelId,
    readiness_receipt: String,
}

/// Machine-projected strict phase. Active is the only variant carrying the
/// opaque receipt accepted by provider-affecting facade operations.
#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExperimentalLiveChannelPhaseStatus {
    Pending,
    Active { activation_receipt: String },
    Revoked,
    Closed,
}

/// Complete stateless custody projection sourced from one machine receipt.
/// Durable target identity remains resolved from `session_id` by the owning
/// Mob host; no caller-supplied identity or mode is trusted here.
#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
#[derive(Clone, PartialEq, Eq)]
pub struct ExperimentalLiveChannelCustodyStatus {
    session_id: SessionId,
    channel_id: LiveChannelId,
    execution_mode: meerkat_core::LiveExecutionMode,
    phase: ExperimentalLiveChannelPhaseStatus,
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
impl std::fmt::Debug for ExperimentalLiveChannelCustodyStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExperimentalLiveChannelCustodyStatus")
            .field("session_id", &"[REDACTED]")
            .field("channel_id", &"[REDACTED]")
            .field("execution_mode", &self.execution_mode)
            .field("phase", &self.phase)
            .finish()
    }
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
impl ExperimentalLiveChannelCustodyStatus {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub const fn execution_mode(&self) -> meerkat_core::LiveExecutionMode {
        self.execution_mode
    }

    #[must_use]
    pub const fn phase(&self) -> &ExperimentalLiveChannelPhaseStatus {
        &self.phase
    }
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
impl ExperimentalLiveChannelPhaseStatus {
    #[must_use]
    pub fn activation_receipt(&self) -> Option<&str> {
        match self {
            Self::Active { activation_receipt } => Some(activation_receipt),
            Self::Pending | Self::Revoked | Self::Closed => None,
        }
    }
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
impl std::fmt::Debug for ExperimentalLivePlaybackOwnerReadiness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExperimentalLivePlaybackOwnerReadiness")
            .field("channel_id", &"[REDACTED]")
            .field("readiness_receipt", &"[REDACTED]")
            .finish()
    }
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
impl ExperimentalLivePlaybackOwnerReadiness {
    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn readiness_receipt(&self) -> &str {
        &self.readiness_receipt
    }
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
impl ExperimentalLiveReplacementRequired {
    #[must_use]
    pub fn open(&self) -> &LiveOpenResult {
        match self {
            Self::CanonicalContext { open, .. } | Self::DelegationResult { open, .. } => open,
        }
    }

    #[must_use]
    pub const fn canonical_seed_cursor(&self) -> u64 {
        match self {
            Self::CanonicalContext {
                canonical_seed_cursor,
                ..
            }
            | Self::DelegationResult {
                canonical_seed_cursor,
                ..
            } => *canonical_seed_cursor,
        }
    }
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
#[derive(Debug, thiserror::Error)]
pub enum ExperimentalLiveContextRecoveryError {
    #[error("failed to close ambiguous live channel: {0}")]
    Close(#[from] ExperimentalLiveChannelCloseError),
    #[error("failed to prepare replacement live channel: {0}")]
    Authority(#[from] crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityError),
    #[error("failed to project canonical replacement seed: {0}")]
    Projection(#[from] RealtimeSessionOpenProjectionError),
    #[error("failed to open exact replacement live channel: {0}")]
    Open(#[from] LiveOpenError),
    #[error("replacement canonical seed cursor did not match generated recovery authority")]
    SeedCursorMismatch,
    #[error("replacement provider binding failed: {binding}; cleanup failed: {cleanup}")]
    BindingCleanup {
        binding: crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
        cleanup: String,
    },
}

#[cfg(feature = "live-webrtc")]
#[derive(Clone, Copy)]
enum LiveWebrtcAnswerDeliveryDisposition {
    Accept,
    Reject,
}

#[cfg(feature = "live-webrtc")]
enum LiveWebrtcPostDeliverySettlement {
    Plain,
    PendingBoundReady {
        pending: ProviderWebrtcPendingBoundReadySeal,
        binder: Arc<dyn LiveWebrtcBoundReadyBinder>,
    },
}

/// Surface-neutral delivery custody for one machine-accepted WebRTC answer.
/// A surface must confirm that its response was published; drop or explicit
/// rejection closes the exact sequence-keyed provider binding.
#[cfg(feature = "live-webrtc")]
pub struct LiveWebrtcAnswerDeliveryCustody {
    decision_tx: tokio::sync::oneshot::Sender<LiveWebrtcAnswerDeliveryDisposition>,
    cleanup_task: tokio::task::JoinHandle<Result<(), String>>,
}

#[cfg(feature = "live-webrtc")]
impl LiveWebrtcAnswerDeliveryCustody {
    pub async fn delivered(self) -> Result<(), LiveWebrtcAnswerCoordinatorError> {
        let Self {
            decision_tx,
            cleanup_task,
        } = self;
        decision_tx
            .send(LiveWebrtcAnswerDeliveryDisposition::Accept)
            .map_err(|_| LiveWebrtcAnswerCoordinatorError::CoordinatorStopped)?;
        Self::await_cleanup(cleanup_task).await
    }

    pub async fn rejected(self) -> Result<(), LiveWebrtcAnswerCoordinatorError> {
        let Self {
            decision_tx,
            cleanup_task,
        } = self;
        decision_tx
            .send(LiveWebrtcAnswerDeliveryDisposition::Reject)
            .map_err(|_| LiveWebrtcAnswerCoordinatorError::CoordinatorStopped)?;
        Self::await_cleanup(cleanup_task).await
    }

    async fn await_cleanup(
        cleanup_task: tokio::task::JoinHandle<Result<(), String>>,
    ) -> Result<(), LiveWebrtcAnswerCoordinatorError> {
        cleanup_task
            .await
            .map_err(|_| LiveWebrtcAnswerCoordinatorError::CoordinatorStopped)?
            .map_err(LiveWebrtcAnswerCoordinatorError::Settlement)
    }
}

#[cfg(feature = "live-webrtc")]
async fn rejected_answer_error_after_cleanup(
    cleanup_task: tokio::task::JoinHandle<Result<(), String>>,
    primary: LiveWebrtcAnswerCoordinatorError,
) -> LiveWebrtcAnswerCoordinatorError {
    match cleanup_task.await {
        Ok(Ok(())) => primary,
        Ok(Err(cleanup)) => LiveWebrtcAnswerCoordinatorError::Settlement(format!(
            "{primary}; rejected-answer cleanup also failed: {cleanup}"
        )),
        Err(join) => LiveWebrtcAnswerCoordinatorError::Settlement(format!(
            "{primary}; rejected-answer cleanup task failed: {join}"
        )),
    }
}

/// Accepted answer plus publication custody. Answer SDP is transport material,
/// while generated authority remains inside the coordinator.
#[cfg(feature = "live-webrtc")]
pub struct CoordinatedLiveWebrtcAnswer {
    pub answer_sdp: String,
    pub session_id: SessionId,
    pub delivery_custody: LiveWebrtcAnswerDeliveryCustody,
}

#[cfg(feature = "live-webrtc")]
#[derive(Debug, thiserror::Error)]
pub enum LiveWebrtcAnswerCoordinatorError {
    #[error("the WebRTC answer channel is not bound")]
    UnboundChannel,
    #[error("WebRTC answer lifecycle authority failed: {0}")]
    LifecycleAuthority(String),
    #[error("WebRTC answer clock failed: {0}")]
    Clock(String),
    #[error("WebRTC answer admission authority failed: {0}")]
    AdmissionAuthority(String),
    #[error("WebRTC answer admission was rejected")]
    AdmissionRejected {
        session_id: SessionId,
        authority: meerkat_runtime::meerkat_machine::LiveWebrtcAnswerAdmissionAuthority,
    },
    #[error("admitted WebRTC answer omitted its generated transport seal")]
    MissingTransportSeal { session_id: SessionId },
    #[error("WebRTC runtime binding authority failed: {0}")]
    RuntimeBindingAuthority(String),
    #[error("WebRTC pending-phase authority failed: {0}")]
    PendingPhaseAuthority(String),
    #[error("WebRTC provider answer failed")]
    TransportRejected {
        session_id: SessionId,
        source: LiveWebrtcError,
        authority: meerkat_runtime::meerkat_machine::LiveChannelRequestRejectionAuthority,
    },
    #[error("WebRTC answer rejection authority failed: {0}")]
    RejectionAuthority(String),
    #[error("WebRTC answer result authority failed: {0}")]
    ResultAuthority(String),
    #[error("WebRTC answer result authority returned a non-answered state")]
    ResultRejected,
    #[error("WebRTC answer coordinator stopped before publishing a result")]
    CoordinatorStopped,
    #[error("WebRTC answer produced bound-ready evidence without a configured binder")]
    MissingBoundReadyBinder,
    #[error("WebRTC bound-ready activation failed: {0}")]
    BoundChannelActivation(String),
    #[error("WebRTC answer settlement failed: {0}")]
    Settlement(String),
}

#[cfg(feature = "live-webrtc")]
#[async_trait]
pub trait LiveWebrtcBoundReadyBinder: Send + Sync {
    async fn bind_answer_ready(
        &self,
        runtime: Arc<MeerkatMachine>,
        binding: &LiveWebrtcBindingRequest,
        receipt: meerkat_live::ProviderWebrtcBoundReadyReceipt,
        answer_observation_sequence: u64,
    ) -> Result<Box<dyn LiveWebrtcBoundReadyCustody>, LiveWebrtcBoundReadyBindFailure>;
}

#[cfg(feature = "live-webrtc")]
pub struct LiveWebrtcBoundReadyBindFailure {
    detail: String,
    rollback: Option<Box<dyn LiveWebrtcBoundReadyCustody>>,
}

#[cfg(feature = "live-webrtc")]
impl LiveWebrtcBoundReadyBindFailure {
    pub(crate) fn before_binding(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            rollback: None,
        }
    }

    pub(crate) fn after_binding(
        detail: impl Into<String>,
        rollback: Box<dyn LiveWebrtcBoundReadyCustody>,
    ) -> Self {
        Self {
            detail: detail.into(),
            rollback: Some(rollback),
        }
    }

    fn into_parts(self) -> (String, Option<Box<dyn LiveWebrtcBoundReadyCustody>>) {
        (self.detail, self.rollback)
    }
}

#[cfg(feature = "live-webrtc")]
#[async_trait]
pub trait LiveWebrtcBoundReadyCustody: Send {
    async fn commit(self: Box<Self>) -> Result<(), String>;
    async fn rollback(self: Box<Self>) -> Result<(), String>;
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
#[derive(Debug, thiserror::Error)]
pub enum ExperimentalLiveChannelCloseError {
    #[error("the experimental live channel is not active for the session")]
    BindingMismatch,
    #[error("experimental live close lifecycle authority failed: {0}")]
    LifecycleAuthority(String),
    #[error("experimental live physical transport authority failed: {0}")]
    PhysicalAuthority(ExperimentalLiveOpenAuthorityError),
    #[error(transparent)]
    Semantic(#[from] LiveChannelVerbError),
}

#[cfg(feature = "live-webrtc")]
fn live_webrtc_answer_rejection_reason(
    error: &LiveWebrtcError,
) -> meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestRejectionReason {
    use meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestRejectionReason;
    match error {
        LiveWebrtcError::ChannelNotFound(_) => LiveChannelRequestRejectionReason::ChannelNotFound,
        LiveWebrtcError::Json(_) => LiveChannelRequestRejectionReason::InvalidPayload,
        _ => LiveChannelRequestRejectionReason::WebrtcAnswerError,
    }
}

#[cfg(feature = "live-webrtc")]
async fn record_live_webrtc_answer_rejection(
    runtime: &MeerkatMachine,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
    error: LiveWebrtcError,
) -> LiveWebrtcAnswerCoordinatorError {
    let rejection = live_webrtc_answer_rejection_reason(&error);
    match runtime
        .resolve_live_channel_request_rejection_reason_result(
            session_id,
            channel_id,
            meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::WebrtcAnswer,
            rejection,
        )
        .await
    {
        Ok(authority) => LiveWebrtcAnswerCoordinatorError::TransportRejected {
            session_id: session_id.clone(),
            source: error,
            authority,
        },
        Err(authority_error) => {
            LiveWebrtcAnswerCoordinatorError::RejectionAuthority(authority_error.to_string())
        }
    }
}

/// One shared answer coordinator for RPC and member/MobKit surfaces.
/// Generated admission is resolved before the provider transport can see the
/// offer, and exact cleanup remains owned until response publication settles.
#[cfg(feature = "live-webrtc")]
pub async fn coordinate_live_webrtc_answer(
    runtime: Arc<MeerkatMachine>,
    answer_transport: Arc<dyn LiveWebrtcAnswerTransport>,
    bound_ready_binder: Option<Arc<dyn LiveWebrtcBoundReadyBinder>>,
    channel_id: LiveChannelId,
    token: String,
    offer_sdp: String,
) -> Result<CoordinatedLiveWebrtcAnswer, LiveWebrtcAnswerCoordinatorError> {
    let session_id = match runtime.live_session_for_webrtc_token(&token).await {
        Some(session_id) => session_id,
        None => runtime
            .live_session_for_active_channel(&channel_id)
            .await
            .ok_or(LiveWebrtcAnswerCoordinatorError::UnboundChannel)?,
    };
    let live_lifecycle_lease = runtime
        .acquire_live_open_lifecycle_lease(&session_id)
        .await
        .map_err(|error| LiveWebrtcAnswerCoordinatorError::LifecycleAuthority(error.to_string()))?;
    let observed_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| LiveWebrtcAnswerCoordinatorError::Clock(error.to_string()))?
        .as_millis()
        .try_into()
        .map_err(|_| {
            LiveWebrtcAnswerCoordinatorError::Clock(
                "system clock milliseconds overflowed u64".to_string(),
            )
        })?;
    let admission = runtime
        .resolve_live_webrtc_answer_admission(&session_id, &channel_id, &token, observed_at_ms)
        .await
        .map_err(|error| LiveWebrtcAnswerCoordinatorError::AdmissionAuthority(error.to_string()))?;
    if !admission.admitted {
        return Err(LiveWebrtcAnswerCoordinatorError::AdmissionRejected {
            session_id,
            authority: admission,
        });
    }
    let runtime_binding = runtime
        .live_webrtc_runtime_binding(&session_id)
        .await
        .map_err(|error| {
            LiveWebrtcAnswerCoordinatorError::RuntimeBindingAuthority(error.to_string())
        })?;
    let transport_seal = admission.transport_seal.ok_or_else(|| {
        LiveWebrtcAnswerCoordinatorError::MissingTransportSeal {
            session_id: session_id.clone(),
        }
    })?;
    let admitted_offer = LiveWebrtcAdmittedOffer::from_machine_admission(
        channel_id.clone(),
        session_id.clone(),
        runtime_binding,
        offer_sdp,
        transport_seal,
    );
    let binding = admitted_offer.binding_request();
    let (answer_tx, answer_rx) = tokio::sync::oneshot::channel();
    let (decision_tx, decision_rx) = tokio::sync::oneshot::channel();
    let (settlement_tx, settlement_rx) =
        tokio::sync::oneshot::channel::<LiveWebrtcPostDeliverySettlement>();
    let (liveness_tx, mut liveness_rx) = tokio::sync::oneshot::channel::<()>();
    let task_runtime = Arc::clone(&runtime);
    let task_transport = Arc::clone(&answer_transport);
    let task_session = session_id.clone();
    let task_channel = channel_id.clone();
    let task_binding: LiveWebrtcBindingRequest = binding.clone();
    let cleanup_task = tokio::spawn(async move {
        let construction_transport = Arc::clone(&task_transport);
        let mut answer_task = tokio::spawn(async move {
            construction_transport
                .answer_admitted_offer(admitted_offer)
                .await
        });
        let (caller_gone, answer_joined) = tokio::select! {
            _ = &mut liveness_rx => {
                answer_task.abort();
                (true, answer_task.await)
            }
            joined = &mut answer_task => (false, joined),
        };
        let cleanup = match answer_joined {
            Ok(Ok(answer)) if caller_gone => task_transport
                .reject_answer(&task_binding, answer.answer_observation_sequence)
                .await
                .map_err(|error| error.to_string()),
            Ok(Ok(answer)) => {
                let sequence = answer.answer_observation_sequence;
                if answer_tx.send(Ok(answer)).is_err() {
                    task_transport
                        .reject_answer(&task_binding, sequence)
                        .await
                        .map_err(|error| error.to_string())
                } else {
                    let settlement = settlement_rx.await.ok();
                    match decision_rx.await {
                        Ok(LiveWebrtcAnswerDeliveryDisposition::Accept) => {
                            let Some(settlement) = settlement else {
                                return task_transport
                                    .reject_answer(&task_binding, sequence)
                                    .await
                                    .map_err(|error| error.to_string())
                                    .and_then(|()| {
                                        Err("answer delivery accepted without settlement custody"
                                            .to_string())
                                    });
                            };
                            match settlement {
                                LiveWebrtcPostDeliverySettlement::Plain => {
                                    task_transport.accept_answer(&task_binding, sequence).await;
                                    Ok(())
                                }
                                LiveWebrtcPostDeliverySettlement::PendingBoundReady {
                                    pending,
                                    binder,
                                } => {
                                    let receipt = match pending
                                        .__resolve_after_answer_delivery()
                                        .await
                                    {
                                        Ok(receipt) => receipt,
                                        Err(error) => {
                                            let physical = task_transport
                                                .reject_answer(&task_binding, sequence)
                                                .await
                                                .map_err(|close| close.to_string());
                                            return match physical {
                                                Ok(()) => Err(format!(
                                                    "provider bound-ready seed failed after answer delivery: {error}"
                                                )),
                                                Err(close) => Err(format!(
                                                    "provider bound-ready seed failed after answer delivery: {error}; exact answer close also failed: {close}"
                                                )),
                                            };
                                        }
                                    };
                                    let custody = match binder
                                        .bind_answer_ready(
                                            Arc::clone(&task_runtime),
                                            &task_binding,
                                            receipt,
                                            sequence,
                                        )
                                        .await
                                    {
                                        Ok(custody) => custody,
                                        Err(error) => {
                                            let (detail, rollback) = error.into_parts();
                                            let semantic = match rollback {
                                                Some(custody) => custody.rollback().await,
                                                None => Ok(()),
                                            };
                                            let physical = task_transport
                                                .reject_answer(&task_binding, sequence)
                                                .await
                                                .map_err(|close| close.to_string());
                                            return match (physical, semantic) {
                                                (Ok(()), Ok(())) => Err(format!(
                                                    "bound-ready activation failed after answer delivery: {detail}"
                                                )),
                                                (Err(close), Ok(())) => Err(format!(
                                                    "bound-ready activation failed after answer delivery: {detail}; exact answer close also failed: {close}"
                                                )),
                                                (Ok(()), Err(rollback)) => Err(format!(
                                                    "bound-ready activation failed after answer delivery: {detail}; semantic rollback also failed: {rollback}"
                                                )),
                                                (Err(close), Err(rollback)) => Err(format!(
                                                    "bound-ready activation failed after answer delivery: {detail}; exact answer close failed: {close}; semantic rollback also failed: {rollback}"
                                                )),
                                            };
                                        }
                                    };
                                    if let Err(error) = custody.commit().await {
                                        let physical = task_transport
                                            .reject_answer(&task_binding, sequence)
                                            .await
                                            .map_err(|close| close.to_string());
                                        return match physical {
                                            Ok(()) => Err(format!(
                                                "bound-ready activation commit failed after answer delivery: {error}"
                                            )),
                                            Err(close) => Err(format!(
                                                "bound-ready activation commit failed after answer delivery: {error}; exact answer close also failed: {close}"
                                            )),
                                        };
                                    }
                                    task_transport.accept_answer(&task_binding, sequence).await;
                                    Ok(())
                                }
                            }
                        }
                        Ok(LiveWebrtcAnswerDeliveryDisposition::Reject) | Err(_) => task_transport
                            .reject_answer(&task_binding, sequence)
                            .await
                            .map_err(|error| error.to_string()),
                    }
                }
            }
            Ok(Err(error)) => {
                let rejection = record_live_webrtc_answer_rejection(
                    &task_runtime,
                    &task_session,
                    &task_channel,
                    error,
                )
                .await;
                let _ = answer_tx.send(Err(rejection));
                Ok(())
            }
            Err(join_error) => {
                if !caller_gone {
                    let rejection = record_live_webrtc_answer_rejection(
                        &task_runtime,
                        &task_session,
                        &task_channel,
                        LiveWebrtcError::PeerCreation {
                            detail: format!("WebRTC answer construction task failed: {join_error}"),
                        },
                    )
                    .await;
                    let _ = answer_tx.send(Err(rejection));
                }
                task_transport
                    .wait_for_construction_cleanup(&task_binding)
                    .await
                    .map_err(|error| error.to_string())
            }
        };
        drop(live_lifecycle_lease);
        cleanup
    });
    let _caller_liveness = liveness_tx;
    let mut answer = match answer_rx.await {
        Ok(result) => result?,
        Err(_) => {
            let _ = cleanup_task.await;
            return Err(LiveWebrtcAnswerCoordinatorError::CoordinatorStopped);
        }
    };
    let sequence = answer.answer_observation_sequence;
    let settlement = if let Some(pending) = answer.pending_bound_ready.take() {
        let Some(binder) = bound_ready_binder else {
            let _ = decision_tx.send(LiveWebrtcAnswerDeliveryDisposition::Reject);
            return Err(rejected_answer_error_after_cleanup(
                cleanup_task,
                LiveWebrtcAnswerCoordinatorError::MissingBoundReadyBinder,
            )
            .await);
        };
        LiveWebrtcPostDeliverySettlement::PendingBoundReady { pending, binder }
    } else {
        let result_authority = runtime
            .resolve_live_webrtc_answer_result(&session_id, &channel_id, sequence)
            .await
            .map_err(|error| LiveWebrtcAnswerCoordinatorError::ResultAuthority(error.to_string()));
        match result_authority {
            Ok(authority)
                if authority.answered
                    && matches!(
                        authority.status,
                        meerkat_runtime::meerkat_machine::dsl::LiveWebrtcAnswerPublicStatus::Answered
                    ) =>
            {
                LiveWebrtcPostDeliverySettlement::Plain
            }
            Ok(_) => {
                let _ = decision_tx.send(LiveWebrtcAnswerDeliveryDisposition::Reject);
                return Err(
                    rejected_answer_error_after_cleanup(
                        cleanup_task,
                        LiveWebrtcAnswerCoordinatorError::ResultRejected,
                    )
                    .await,
                );
            }
            Err(error) => {
                let _ = decision_tx.send(LiveWebrtcAnswerDeliveryDisposition::Reject);
                return Err(rejected_answer_error_after_cleanup(cleanup_task, error).await);
            }
        }
    };
    if settlement_tx.send(settlement).is_err() {
        let _ = decision_tx.send(LiveWebrtcAnswerDeliveryDisposition::Reject);
        let _ = cleanup_task.await;
        return Err(LiveWebrtcAnswerCoordinatorError::CoordinatorStopped);
    }
    Ok(CoordinatedLiveWebrtcAnswer {
        answer_sdp: answer.answer_sdp,
        session_id,
        delivery_custody: LiveWebrtcAnswerDeliveryCustody {
            decision_tx,
            cleanup_task,
        },
    })
}

/// Fail-closed member-host ingress hook (DEC-P6B-L5): bridge live commands
/// are member-addressed and a member session's peer ingress is mob-owned —
/// the exact §16.5 invariant — so the session-owned branch is unreachable
/// by construction on this surface. Reaching it is a composition fault,
/// never a silent skip.
pub struct MobOwnedOnlyIngress;

#[async_trait]
impl LiveSessionIngressReconciler for MobOwnedOnlyIngress {
    async fn ensure_session_owned_live_ingress(
        &self,
        session_id: &SessionId,
    ) -> Result<(), LiveIngressError> {
        Err(LiveIngressError::Internal(format!(
            "member-host live open reached session-owned peer ingress for {session_id}; \
             member sessions are mob-owned by construction"
        )))
    }
}

static MOB_OWNED_ONLY_INGRESS: MobOwnedOnlyIngress = MobOwnedOnlyIngress;

/// Construction material for [`ServiceMemberLiveHost`] (DEC-P6B-L10).
///
/// Honest-by-construction field choices for the host role: the staged
/// registry and capacity ledger start EMPTY (the host never defers session
/// creation — members materialize through `MaterializeMember`, so the
/// pipeline's staged branches structurally never fire); recovery uses
/// `LocalResources` with no default client/decorator/external tools
/// (members carry their own identities); realm facts are the daemon's own
/// (recovery correctness).
pub struct ServiceMemberLiveHostConfig<B: SessionAgentBuilder + 'static = FactoryAgentBuilder> {
    /// The daemon's runtime-backed session service.
    pub service: Arc<PersistentSessionService<B>>,
    /// The daemon's `MeerkatMachine`.
    pub runtime_adapter: Arc<MeerkatMachine>,
    /// The composed live adapter host.
    pub host: Arc<LiveAdapterHost>,
    /// The composed live WS transport state (token mint).
    /// Optional member-open WebSocket transport. Cleanup/status do not depend
    /// on it, so WebRTC-only session compositions still install this host as
    /// the transport-neutral lifecycle cleanup seam.
    pub ws_state: Option<Arc<LiveWsState>>,
    /// Validated `--live-ws-advertise` absolute base URL (DL6).
    pub base_url: Option<String>,
    /// Per-open realtime session factory over the host's own realm chain.
    pub session_factory: Arc<dyn RealtimeSessionFactory>,
    /// The daemon's realm id.
    pub realm_id: Option<RealmId>,
    /// The daemon's instance id.
    pub instance_id: Option<String>,
    /// The daemon's backend label.
    pub backend: Option<String>,
}

/// Owned facade wrapper implementing the machine-injected
/// [`MemberLiveHost`] over the ONE extracted live pipeline (ADJ-P6B-1 /
/// DEC-P6B-L1: the pipeline stays a borrowing struct; this wrapper builds
/// it per trait call from its `Arc`ed composition). The builder parameter
/// admits any shared [`SessionAgentBuilder`] service while defaulting to
/// [`FactoryAgentBuilder`] for existing callers.
pub struct ServiceMemberLiveHost<B: SessionAgentBuilder + 'static = FactoryAgentBuilder> {
    service: Arc<PersistentSessionService<B>>,
    staged_sessions: Arc<StagedSessionRegistry>,
    staged_capacity_admissions: StagedCapacityAdmissions,
    runtime_adapter: Arc<MeerkatMachine>,
    actor_witness_slots: Arc<
        std::sync::Mutex<std::collections::HashMap<SessionId, crate::LiveSessionActorWitnessSlot>>,
    >,
    host: Arc<LiveAdapterHost>,
    ws_state: Option<Arc<LiveWsState>>,
    base_url: Option<String>,
    #[cfg(feature = "live-webrtc")]
    webrtc_state: Option<Arc<meerkat_live::LiveWebrtcState>>,
    session_factory: Arc<dyn RealtimeSessionFactory>,
    realm_id: Option<RealmId>,
    instance_id: Option<String>,
    backend: Option<String>,
}

/// Shared production composition for canonical context mirroring. It wraps
/// the owning host's delegation activator, retains exact provider controls by
/// generated channel identity, and publishes typed replacement-required
/// bootstraps after ambiguous delivery.
#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
pub struct ExperimentalGptLiveContextMirrorHost<
    B: SessionAgentBuilder + 'static = FactoryAgentBuilder,
> {
    runtime: Arc<MeerkatMachine>,
    member_host: Arc<ServiceMemberLiveHost<B>>,
    open_authority: Arc<dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider>,
    downstream_activator:
        Arc<dyn crate::experimental_gpt_live::ExperimentalLiveBoundChannelActivator>,
    controls: tokio::sync::Mutex<
        std::collections::HashMap<
            (SessionId, LiveChannelId),
            Arc<dyn crate::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        >,
    >,
    replacements: tokio::sync::Mutex<
        std::collections::HashMap<SessionId, ExperimentalLiveReplacementRequired>,
    >,
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
impl<B: SessionAgentBuilder + 'static> ExperimentalGptLiveContextMirrorHost<B> {
    #[must_use]
    pub fn new(
        runtime: Arc<MeerkatMachine>,
        member_host: Arc<ServiceMemberLiveHost<B>>,
        open_authority: Arc<
            dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        >,
        downstream_activator: Arc<
            dyn crate::experimental_gpt_live::ExperimentalLiveBoundChannelActivator,
        >,
    ) -> Arc<Self> {
        let host = Arc::new(Self {
            runtime: Arc::clone(&runtime),
            member_host,
            open_authority,
            downstream_activator,
            controls: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            replacements: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        });
        runtime.set_live_context_mirror_host(Arc::clone(&host)
            as Arc<dyn meerkat_runtime::live_context_mirror::LiveContextMirrorHost>);
        host
    }

    /// Read the pending client renegotiation bootstrap for this session. The
    /// exact same value remains available until its replacement answer binds.
    pub async fn pending_replacement_required(
        &self,
        session_id: &SessionId,
    ) -> Option<ExperimentalLiveReplacementRequired> {
        self.replacements.lock().await.get(session_id).cloned()
    }
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
#[async_trait]
impl<B: SessionAgentBuilder + 'static>
    crate::experimental_gpt_live::ExperimentalLiveBoundChannelActivator
    for ExperimentalGptLiveContextMirrorHost<B>
{
    async fn prepare_bound_channel(
        &self,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn crate::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
    ) -> Result<(), String> {
        let key = (binding.session_id().clone(), binding.channel_id().clone());
        self.controls
            .lock()
            .await
            .insert(key.clone(), Arc::clone(&control));
        if let Err(error) = self
            .downstream_activator
            .prepare_bound_channel(binding.clone(), control)
            .await
        {
            self.controls.lock().await.remove(&key);
            return Err(error);
        }
        Ok(())
    }

    async fn run_bound_channel(
        &self,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn crate::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
    ) {
        let key = (binding.session_id().clone(), binding.channel_id().clone());
        let caught_up = self
            .member_host
            .catch_up_live_context_after_bind(&binding)
            .await
            .is_ok();
        if caught_up {
            let mut replacements = self.replacements.lock().await;
            if replacements
                .get(binding.session_id())
                .is_some_and(|replacement| {
                    replacement.open().channel_id == binding.channel_id().as_str()
                })
            {
                replacements.remove(binding.session_id());
            }
            drop(replacements);
            self.downstream_activator
                .run_bound_channel(binding.clone(), control)
                .await;
        } else {
            let _ = self
                .downstream_activator
                .deactivate_bound_channel(&binding)
                .await;
        }
        self.controls.lock().await.remove(&key);
    }

    async fn observe_provider_lifecycle(
        &self,
        observation: &meerkat_live::LiveSidebandObservation,
    ) -> Result<(), String> {
        if matches!(
            observation.kind(),
            meerkat_live::LiveSidebandObservationKind::TurnStarted {
                role: meerkat_live::LiveSidebandTurnRole::Assistant,
                ..
            }
        ) {
            self.runtime
                .observe_live_assistant_turn_started(observation)
                .await
                .map_err(|error| error.to_string())?;
        }
        self.downstream_activator
            .observe_provider_lifecycle(observation)
            .await
    }

    async fn deactivate_bound_channel(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<(), String> {
        self.downstream_activator
            .deactivate_bound_channel(binding)
            .await?;
        self.controls
            .lock()
            .await
            .remove(&(binding.session_id().clone(), binding.channel_id().clone()));
        Ok(())
    }

    async fn retire_bound_channel_after_pump_exit(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<(), crate::experimental_gpt_live::ExperimentalLivePumpRetirementError> {
        let close = self
            .member_host
            .close_live_channel(Some(self.open_authority.as_ref()), binding.channel_id())
            .await;
        match close {
            Ok(_) | Err(ExperimentalLiveChannelCloseError::PhysicalAuthority(_)) => {}
            Err(ExperimentalLiveChannelCloseError::BindingMismatch)
                if self
                    .runtime
                    .live_session_for_active_channel(binding.channel_id())
                    .await
                    .is_none() =>
            {
                // An exact concurrent close already committed and unbound
                // this channel. Pump retirement is idempotently complete.
            }
            Err(error) => {
                return Err(
                    crate::experimental_gpt_live::ExperimentalLivePumpRetirementError::SemanticUncommitted(
                        error.to_string(),
                    ),
                );
            }
        }
        self.runtime
            .retire_live_assistant_output_handles(binding.session_id(), binding.channel_id());
        Ok(())
    }

    async fn pending_replacement_required(
        &self,
        session_id: &SessionId,
    ) -> Option<ExperimentalLiveReplacementRequired> {
        ExperimentalGptLiveContextMirrorHost::pending_replacement_required(self, session_id).await
    }
}

#[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
#[async_trait]
impl<B: SessionAgentBuilder + 'static> meerkat_runtime::live_context_mirror::LiveContextMirrorHost
    for ExperimentalGptLiveContextMirrorHost<B>
{
    async fn append_context(
        &self,
        authority: meerkat_runtime::live_execution::LiveContextAppendAuthority,
        context: String,
    ) -> Result<
        (
            meerkat_runtime::live_execution::LiveContextAppendAuthority,
            meerkat_core::LiveAppendDeliveryOutcome,
        ),
        String,
    > {
        let key = (
            authority.session_id().clone(),
            authority.channel_id().clone(),
        );
        let Some(control) = self.controls.lock().await.get(&key).cloned() else {
            return Ok((
                authority,
                meerkat_core::LiveAppendDeliveryOutcome::Ambiguous,
            ));
        };
        // Keep the same one-use authority custody for terminal resolution if
        // provider mechanics fail without a typed delivery observation. The
        // conservative outcome is ambiguity, which generated state turns into
        // close/reseed authority and never into a blind retry.
        let unresolved_authority = authority.clone();
        let dispatch = match control.append_session_context(authority, context).await {
            Ok(dispatch) => dispatch,
            Err(_) => {
                return Ok((
                    unresolved_authority,
                    meerkat_core::LiveAppendDeliveryOutcome::Ambiguous,
                ));
            }
        };
        let resolution = match dispatch {
            crate::experimental_gpt_live::ExperimentalGptLiveAppendDispatch::Resolved(
                resolution,
            ) => resolution,
            crate::experimental_gpt_live::ExperimentalGptLiveAppendDispatch::AwaitingAcknowledgement(
                waiter,
            ) => waiter.resolve().await.map_err(|error| error.to_string())?,
        };
        Ok(resolution.into_parts())
    }

    async fn recover_ambiguous_append(
        &self,
        authority: meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
    ) -> Result<(), String> {
        let session_id = authority.session_id().clone();
        self.controls
            .lock()
            .await
            .remove(&(session_id.clone(), authority.closing_channel_id().clone()));
        let replacement = self
            .member_host
            .open_live_context_replacement(self.open_authority.as_ref(), authority)
            .await
            .map_err(|error| error.to_string())?;
        self.replacements
            .lock()
            .await
            .insert(session_id, replacement);
        Ok(())
    }

    async fn recover_ambiguous_delegation_result(
        &self,
        authority: meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
    ) -> Result<(), String> {
        let session_id = authority.session_id().clone();
        self.controls
            .lock()
            .await
            .remove(&(session_id.clone(), authority.closing_channel_id().clone()));
        let replacement = self
            .member_host
            .open_live_result_replacement(self.open_authority.as_ref(), authority)
            .await
            .map_err(|error| error.to_string())?;
        self.replacements
            .lock()
            .await
            .insert(session_id, replacement);
        Ok(())
    }
}

impl<B: SessionAgentBuilder + 'static> ServiceMemberLiveHost<B> {
    #[must_use]
    pub fn new(config: ServiceMemberLiveHostConfig<B>) -> Self {
        Self {
            service: config.service,
            staged_sessions: Arc::new(StagedSessionRegistry::new()),
            staged_capacity_admissions: Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            runtime_adapter: config.runtime_adapter,
            actor_witness_slots: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            host: config.host,
            ws_state: config.ws_state,
            base_url: config.base_url,
            #[cfg(feature = "live-webrtc")]
            webrtc_state: None,
            session_factory: config.session_factory,
            realm_id: config.realm_id,
            instance_id: config.instance_id,
            backend: config.backend,
        }
    }

    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    async fn catch_up_live_context_after_bind(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<(), String> {
        let (committed, authority_token) = self
            .service
            .export_live_context_committed_boundary(binding.session_id())
            .await
            .map_err(|error| error.to_string())?;
        self.runtime_adapter
            .enqueue_committed_parent_session_boundary(
                binding.session_id(),
                &committed,
                &authority_token,
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn orchestrator(&self) -> LiveOrchestrator<'_, B> {
        let actor_witness_slots = Arc::clone(&self.actor_witness_slots);
        LiveOrchestrator {
            service: &self.service,
            staged_sessions: &self.staged_sessions,
            staged_capacity_admissions: &self.staged_capacity_admissions,
            runtime_adapter: &self.runtime_adapter,
            host: Some(Arc::clone(&self.host)),
            config_runtime: None,
            default_llm_client: None,
            agent_llm_client_decorator: None,
            external_tools: None,
            archive_runtime_cleanup: ArchiveRuntimeCleanup {
                runtime_adapter: Arc::clone(&self.runtime_adapter),
                pending_session_event_streams: None,
                mcp_state: None,
                mob_state: None,
            },
            realm_id: self.realm_id.as_ref(),
            instance_id: self.instance_id.as_deref(),
            backend: self.backend.as_deref(),
            ingress_reconciler: Some(&MOB_OWNED_ONLY_INGRESS),
            actor_witness_capture: Arc::new(move |session_id, actor_witness_slot| {
                actor_witness_slots
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(session_id, actor_witness_slot);
            }),
        }
    }

    /// Attach transport cleanup without widening member-addressed Open
    /// policy: WebRTC remains a session-scoped transport, but lifecycle must
    /// still physically close its peer before terminalizing a member session.
    #[cfg(feature = "live-webrtc")]
    #[must_use]
    pub fn with_webrtc_cleanup_state(mut self, state: Arc<meerkat_live::LiveWebrtcState>) -> Self {
        self.webrtc_state = Some(state);
        self
    }

    /// Complete a WebRTC bootstrap through the same machine-admitted answer
    /// coordinator used by RPC. MobKit/member surfaces supply only their
    /// composed transport and foreign offer material.
    #[cfg(feature = "live-webrtc")]
    pub async fn answer_webrtc_offer(
        &self,
        answer_transport: Arc<dyn LiveWebrtcAnswerTransport>,
        bound_ready_binder: Option<Arc<dyn LiveWebrtcBoundReadyBinder>>,
        channel_id: LiveChannelId,
        token: String,
        offer_sdp: String,
    ) -> Result<CoordinatedLiveWebrtcAnswer, LiveWebrtcAnswerCoordinatorError> {
        coordinate_live_webrtc_answer(
            Arc::clone(&self.runtime_adapter),
            answer_transport,
            bound_ready_binder,
            channel_id,
            token,
            offer_sdp,
        )
        .await
    }

    /// Register the sole playback owner against the exact current pending
    /// receipt. The returned receipt is an opaque projection; the sealed
    /// authority is reacquired from the machine before answer IO.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn register_experimental_live_playback_owner(
        &self,
        channel_id: &LiveChannelId,
        pending_receipt: &str,
    ) -> Result<ExperimentalLivePlaybackOwnerReadiness, meerkat_runtime::RuntimeDriverError> {
        let session_id = self
            .experimental_live_session_for_channel(channel_id)
            .await?;
        let stage = self
            .runtime_adapter
            .validate_live_pending_channel_receipt(&session_id, channel_id, pending_receipt)
            .await?;
        let readiness = self
            .runtime_adapter
            .register_live_playback_owner(&stage, &uuid::Uuid::new_v4().to_string())
            .await?;
        Ok(ExperimentalLivePlaybackOwnerReadiness {
            channel_id: channel_id.clone(),
            readiness_receipt: readiness.readiness_id().to_string(),
        })
    }

    /// Answer a strict pending channel only after reacquiring exact pending
    /// and playback-owner authority. No provider IO begins on a caller-only
    /// channel or receipt assertion.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn answer_experimental_live_webrtc_offer(
        &self,
        answer_transport: Arc<dyn LiveWebrtcAnswerTransport>,
        bound_ready_binder: Arc<dyn LiveWebrtcBoundReadyBinder>,
        channel_id: LiveChannelId,
        pending_receipt: &str,
        readiness_receipt: &str,
        token: String,
        offer_sdp: String,
    ) -> Result<CoordinatedLiveWebrtcAnswer, LiveWebrtcAnswerCoordinatorError> {
        let session_id = self
            .experimental_live_session_for_channel(&channel_id)
            .await
            .map_err(|error| {
                LiveWebrtcAnswerCoordinatorError::PendingPhaseAuthority(error.to_string())
            })?;
        self.runtime_adapter
            .validate_live_playback_owner_readiness(
                &session_id,
                &channel_id,
                pending_receipt,
                readiness_receipt,
            )
            .await
            .map_err(|error| {
                LiveWebrtcAnswerCoordinatorError::PendingPhaseAuthority(error.to_string())
            })?;
        self.answer_webrtc_offer(
            answer_transport,
            Some(bound_ready_binder),
            channel_id,
            token,
            offer_sdp,
        )
        .await
    }

    /// Read the machine-owned strict phase without maintaining a facade
    /// mirror.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn experimental_live_channel_phase(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<Option<meerkat_core::LiveExecutionChannelPhase>, meerkat_runtime::RuntimeDriverError>
    {
        let session_id = self
            .experimental_live_session_for_channel(channel_id)
            .await?;
        self.runtime_adapter
            .live_execution_channel_phase(&session_id, channel_id)
            .await
    }

    /// Reacquire strict channel custody from the original opaque pending
    /// receipt. Active custody carries the exact machine activation receipt.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn validate_experimental_live_channel_custody(
        &self,
        channel_id: &LiveChannelId,
        pending_receipt: &str,
    ) -> Result<ExperimentalLiveChannelCustodyStatus, meerkat_runtime::RuntimeDriverError> {
        let session_id = self
            .experimental_live_session_for_channel(channel_id)
            .await?;
        let projection = self
            .runtime_adapter
            .validate_live_channel_custody_by_pending_receipt(
                &session_id,
                channel_id,
                pending_receipt,
            )
            .await?;
        Ok(Self::project_experimental_live_custody(projection))
    }

    /// Reacquire strict channel custody from an exact active receipt.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn validate_experimental_live_channel_custody_by_activation(
        &self,
        channel_id: &LiveChannelId,
        activation_receipt: &str,
    ) -> Result<ExperimentalLiveChannelCustodyStatus, meerkat_runtime::RuntimeDriverError> {
        let session_id = self
            .experimental_live_session_for_channel(channel_id)
            .await?;
        let projection = self
            .runtime_adapter
            .validate_live_channel_custody_by_activation_receipt(
                &session_id,
                channel_id,
                activation_receipt,
            )
            .await?;
        Ok(Self::project_experimental_live_custody(projection))
    }

    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    fn project_experimental_live_custody(
        projection: meerkat_runtime::meerkat_machine::LiveChannelCustodyProjection,
    ) -> ExperimentalLiveChannelCustodyStatus {
        let phase = match projection.state() {
            meerkat_runtime::meerkat_machine::LiveChannelCustodyState::Pending(_) => {
                ExperimentalLiveChannelPhaseStatus::Pending
            }
            meerkat_runtime::meerkat_machine::LiveChannelCustodyState::Active(activation) => {
                ExperimentalLiveChannelPhaseStatus::Active {
                    activation_receipt: activation.activation_receipt().to_string(),
                }
            }
            meerkat_runtime::meerkat_machine::LiveChannelCustodyState::Revoked => {
                ExperimentalLiveChannelPhaseStatus::Revoked
            }
            meerkat_runtime::meerkat_machine::LiveChannelCustodyState::Closed => {
                ExperimentalLiveChannelPhaseStatus::Closed
            }
        };
        ExperimentalLiveChannelCustodyStatus {
            session_id: projection.session_id().clone(),
            channel_id: projection.channel_id().clone(),
            execution_mode: projection.mode(),
            phase,
        }
    }

    /// Reacquire the exact active receipt. Callers receive no authority they
    /// can use for provider IO; provider-affecting facade methods authorize
    /// and consume their own one-use control immediately before dispatch.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn validate_experimental_live_activation(
        &self,
        channel_id: &LiveChannelId,
        activation_receipt: &str,
    ) -> Result<(), meerkat_runtime::RuntimeDriverError> {
        let session_id = self
            .experimental_live_session_for_channel(channel_id)
            .await?;
        self.runtime_adapter
            .validate_live_channel_activation_receipt(&session_id, channel_id, activation_receipt)
            .await
            .map(|_| ())
    }

    /// Revoke the exact playback owner on local owner loss. The machine owns
    /// the phase transition; this never retires the durable Mob member.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn revoke_experimental_live_playback_owner(
        &self,
        channel_id: &LiveChannelId,
        pending_receipt: &str,
        readiness_receipt: &str,
        activation_receipt: Option<&str>,
    ) -> Result<(), meerkat_runtime::RuntimeDriverError> {
        let session_id = self
            .experimental_live_session_for_channel(channel_id)
            .await?;
        let readiness = match activation_receipt {
            Some(activation_receipt) => {
                self.runtime_adapter
                    .validate_live_active_playback_owner_readiness(
                        &session_id,
                        channel_id,
                        activation_receipt,
                        pending_receipt,
                        readiness_receipt,
                    )
                    .await?
            }
            None => {
                self.runtime_adapter
                    .validate_live_playback_owner_readiness(
                        &session_id,
                        channel_id,
                        pending_receipt,
                        readiness_receipt,
                    )
                    .await?
            }
        };
        self.runtime_adapter
            .revoke_live_playback_owner(&readiness)
            .await
            .map(|_| ())
    }

    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    async fn experimental_live_session_for_channel(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<SessionId, meerkat_runtime::RuntimeDriverError> {
        self.runtime_adapter
            .live_session_for_status_channel(channel_id)
            .await
            .ok_or_else(|| meerkat_runtime::RuntimeDriverError::ValidationFailed {
                reason: "experimental live channel has no current machine-owned session"
                    .to_string(),
            })
    }

    fn transport_context(&self) -> LiveTransportContext<'_> {
        let context = LiveTransportContext::new(self.ws_state.as_deref(), self.base_url.as_deref());
        #[cfg(feature = "live-webrtc")]
        let context = context.with_webrtc(self.webrtc_state.as_deref());
        context
    }

    /// Resolve an experimental assistant output through its machine-sealed,
    /// public-safe address. Provider response and item identifiers never
    /// cross this facade.
    #[cfg(feature = "experimental-gpt-live")]
    pub async fn truncate_live_output(
        &self,
        channel_id: &LiveChannelId,
        activation_receipt: &str,
        output_id: &str,
        audio_played_ms: u64,
        reported_playback_prefix: Option<String>,
    ) -> Result<meerkat_contracts::LiveTruncateResult, LiveChannelVerbError> {
        self.consume_experimental_live_active_control(
            channel_id,
            activation_receipt,
            "truncate_live_output",
        )
        .await?;
        self.orchestrator()
            .truncate_live_output(
                &self.host,
                self.transport_context(),
                channel_id,
                None,
                crate::session_runtime::live_orchestration::LiveTruncateCursor {
                    output_id: Some(output_id.to_string()),
                    item_id: None,
                    content_index: None,
                    audio_played_ms,
                    reported_playback_prefix,
                },
            )
            .await
    }

    /// Commit the full staged assistant final only after the playback owner
    /// reports exact completion for the machine-sealed output address.
    #[cfg(feature = "experimental-gpt-live")]
    pub async fn complete_live_playback(
        &self,
        channel_id: &LiveChannelId,
        activation_receipt: &str,
        output_id: &str,
    ) -> Result<meerkat_contracts::LivePlaybackCompleteResult, LiveChannelVerbError> {
        self.consume_experimental_live_active_control(
            channel_id,
            activation_receipt,
            "complete_live_playback",
        )
        .await?;
        self.orchestrator()
            .complete_live_playback(&self.host, channel_id, None, output_id)
            .await
    }

    /// Send one full-duplex input chunk only after exact active receipt
    /// validation and one-use control consumption immediately before the
    /// shared provider IO path.
    #[cfg(feature = "experimental-gpt-live")]
    pub async fn send_experimental_live_input(
        &self,
        channel_id: &LiveChannelId,
        activation_receipt: &str,
        chunk: LiveInputChunk,
    ) -> Result<meerkat_contracts::LiveSendInputResult, LiveChannelVerbError> {
        self.consume_experimental_live_active_control(
            channel_id,
            activation_receipt,
            "send_live_input",
        )
        .await?;
        self.orchestrator()
            .send_live_input(&self.host, channel_id, None, chunk)
            .await
    }

    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    async fn consume_experimental_live_active_control(
        &self,
        channel_id: &LiveChannelId,
        activation_receipt: &str,
        operation: &str,
    ) -> Result<(), LiveChannelVerbError> {
        let session_id = self
            .runtime_adapter
            .live_session_for_active_channel(channel_id)
            .await
            .ok_or_else(|| LiveChannelVerbError::HostCommit {
                message: "experimental live channel is not active".to_string(),
            })?;
        let activation = self
            .runtime_adapter
            .validate_live_channel_activation_receipt(&session_id, channel_id, activation_receipt)
            .await
            .map_err(|error| LiveChannelVerbError::HostCommit {
                message: format!("active live receipt rejected: {error}"),
            })?;
        let authority = self
            .runtime_adapter
            .authorize_live_active_channel_control(&activation, operation)
            .await
            .map_err(|error| LiveChannelVerbError::HostCommit {
                message: format!("active live control rejected: {error}"),
            })?;
        self.runtime_adapter
            .consume_live_active_channel_control(authority)
            .await
            .map_err(|error| LiveChannelVerbError::HostCommit {
                message: format!("active live control dispatch rejected: {error}"),
            })?;
        Ok(())
    }

    /// Execute one provider-affecting control only after consuming exact
    /// active-channel authority immediately before the shared IO path.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn control_experimental_live_channel(
        &self,
        channel_id: &LiveChannelId,
        activation_receipt: &str,
        verb: BridgeLiveControlVerb,
    ) -> Result<BridgeLiveControlOutcome, LiveChannelVerbError> {
        self.consume_experimental_live_active_control(
            channel_id,
            activation_receipt,
            "control_live_channel",
        )
        .await?;
        self.orchestrator()
            .control_live_channel(&self.host, self.transport_context(), channel_id, None, verb)
            .await
    }

    /// Build the typed, session-sealed projection used by a composed surface
    /// before it enters the shared effectful S5-S12 open pipeline.
    #[doc(hidden)]
    pub async fn prepare_open_projection(
        &self,
        session: &SessionId,
        turning_mode: RealtimeTurningMode,
        seed_window: Option<LiveSeedWindow>,
    ) -> Result<RealtimeSessionOpenProjection, RealtimeSessionOpenProjectionError> {
        self.orchestrator()
            .live_open_projection_for_session(session, turning_mode, seed_window)
            .await
    }

    /// Continue a composed surface's typed projection through the exact
    /// shared S5-S12 member-open pipeline.
    #[doc(hidden)]
    pub async fn open_from_projection(
        &self,
        session: &SessionId,
        projection: RealtimeSessionOpenProjection,
        transport: Option<LiveOpenTransport>,
    ) -> Result<LiveOpenResult, LiveOpenError> {
        self.orchestrator()
            .open_live_channel_from_projection(
                &self.host,
                self.transport_context(),
                self.session_factory.as_ref(),
                session,
                projection,
                transport,
            )
            .await
    }

    /// Open a strict channel-scoped execution identity through Meerkat's one
    /// shared prepare/project/S5-S12/bind/cleanup coordinator.
    #[cfg(feature = "experimental-gpt-live")]
    pub async fn open_with_execution_identity(
        &self,
        authority: &dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        session: &SessionId,
        execution_identity: &WireLiveExecutionIdentityOverrideV1,
        turning_mode: Option<RealtimeTurningMode>,
        seed_window: Option<LiveSeedWindow>,
        transport: Option<LiveOpenTransport>,
    ) -> Result<ExperimentalLivePendingChannel, ExperimentalLiveChannelOpenError> {
        self.orchestrator()
            .open_live_channel_with_execution_identity(
                &self.host,
                self.transport_context(),
                authority,
                session,
                execution_identity,
                turning_mode,
                seed_window,
                transport,
            )
            .await
    }

    /// Realize generated ambiguity recovery without pretending an existing
    /// browser WebRTC peer can reconnect transparently. This closes the exact
    /// old channel, opens the generated replacement identity, and returns the
    /// fresh signaling bootstrap. Generated execution binding remains pending
    /// until the client's new answer acknowledges the canonical seed.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn open_live_context_replacement(
        &self,
        authority: &dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        recovery: meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
    ) -> Result<ExperimentalLiveReplacementRequired, ExperimentalLiveContextRecoveryError> {
        let profile_id = authority
            .bound_execution_profile_id(recovery.closing_channel_id(), recovery.session_id())
            .await?;
        self.close_live_channel(Some(authority), recovery.closing_channel_id())
            .await?;

        let execution_identity = WireLiveExecutionIdentityOverrideV1 {
            version: WireLiveExecutionIdentityVersion::V1,
            profile_id,
        };
        let pending = authority
            .prepare_open(recovery.session_id(), &execution_identity)
            .await?;
        let mut projection = self
            .prepare_open_projection(
                recovery.session_id(),
                RealtimeTurningMode::ProviderManaged,
                None,
            )
            .await?;
        if projection.open_config.canonical_message_cursor() != recovery.canonical_seed_cursor() {
            return Err(ExperimentalLiveContextRecoveryError::SeedCursorMismatch);
        }
        pending.apply_execution_identity(&mut projection);
        let result = self
            .orchestrator()
            .open_live_channel_from_projection_for_recovery(
                &self.host,
                self.transport_context(),
                pending.session_factory(),
                recovery.session_id(),
                projection,
                Some(LiveOpenTransport::Webrtc),
                &recovery,
            )
            .await?;

        let replacement_channel_id = LiveChannelId::new(&result.channel_id);
        if self
            .runtime_adapter
            .stage_experimental_live_execution(
                recovery.session_id(),
                &replacement_channel_id,
                recovery.canonical_seed_cursor(),
            )
            .await
            .is_err()
        {
            let binding =
                crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::ChannelBindingFailed;
            authority
                .unbind_channel(&replacement_channel_id, recovery.session_id())
                .await;
            if let Err(cleanup) = self
                .orchestrator()
                .close_live_channel(
                    &self.host,
                    &replacement_channel_id,
                    Some(recovery.session_id()),
                )
                .await
            {
                return Err(ExperimentalLiveContextRecoveryError::BindingCleanup {
                    binding,
                    cleanup: cleanup.to_string(),
                });
            }
            return Err(ExperimentalLiveContextRecoveryError::Authority(binding));
        }

        if let Err(binding) = authority
            .register_context_recovery_for_answer(recovery.clone())
            .await
        {
            authority
                .unbind_channel(&replacement_channel_id, recovery.session_id())
                .await;
            if let Err(cleanup) = self
                .orchestrator()
                .close_live_channel(
                    &self.host,
                    &replacement_channel_id,
                    Some(recovery.session_id()),
                )
                .await
            {
                return Err(ExperimentalLiveContextRecoveryError::BindingCleanup {
                    binding,
                    cleanup: cleanup.to_string(),
                });
            }
            return Err(ExperimentalLiveContextRecoveryError::Authority(binding));
        }
        if let Err(binding) = pending.bind_opened(&result).await {
            authority
                .unbind_channel(&replacement_channel_id, recovery.session_id())
                .await;
            let cleanup = self
                .orchestrator()
                .close_live_channel(
                    &self.host,
                    &replacement_channel_id,
                    Some(recovery.session_id()),
                )
                .await
                .map(|_| ())
                .map_err(|error| error.to_string());
            return Err(ExperimentalLiveContextRecoveryError::BindingCleanup {
                binding,
                cleanup: cleanup.err().unwrap_or_default(),
            });
        }
        Ok(ExperimentalLiveReplacementRequired::CanonicalContext {
            open: result,
            canonical_seed_cursor: recovery.canonical_seed_cursor(),
        })
    }

    /// Realize generated delegation-result ambiguity recovery through the
    /// same exact close, fresh open, seed, and answer bootstrap choreography
    /// as context recovery, while retaining the distinct result authority.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn open_live_result_replacement(
        &self,
        authority: &dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        recovery: meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
    ) -> Result<ExperimentalLiveReplacementRequired, ExperimentalLiveContextRecoveryError> {
        let profile_id = authority
            .bound_execution_profile_id(recovery.closing_channel_id(), recovery.session_id())
            .await?;
        self.close_live_channel(Some(authority), recovery.closing_channel_id())
            .await?;

        let execution_identity = WireLiveExecutionIdentityOverrideV1 {
            version: WireLiveExecutionIdentityVersion::V1,
            profile_id,
        };
        let pending = authority
            .prepare_open(recovery.session_id(), &execution_identity)
            .await?;
        let mut projection = self
            .prepare_open_projection(
                recovery.session_id(),
                RealtimeTurningMode::ProviderManaged,
                None,
            )
            .await?;
        if projection.open_config.canonical_message_cursor() != recovery.canonical_seed_cursor() {
            return Err(ExperimentalLiveContextRecoveryError::SeedCursorMismatch);
        }
        pending.apply_execution_identity(&mut projection);
        let result = self
            .orchestrator()
            .open_live_channel_from_projection_for_result_recovery(
                &self.host,
                self.transport_context(),
                pending.session_factory(),
                recovery.session_id(),
                projection,
                Some(LiveOpenTransport::Webrtc),
                &recovery,
            )
            .await?;

        let replacement_channel_id = LiveChannelId::new(&result.channel_id);
        if self
            .runtime_adapter
            .stage_experimental_live_execution(
                recovery.session_id(),
                &replacement_channel_id,
                recovery.canonical_seed_cursor(),
            )
            .await
            .is_err()
        {
            let binding =
                crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::ChannelBindingFailed;
            authority
                .unbind_channel(&replacement_channel_id, recovery.session_id())
                .await;
            if let Err(cleanup) = self
                .orchestrator()
                .close_live_channel(
                    &self.host,
                    &replacement_channel_id,
                    Some(recovery.session_id()),
                )
                .await
            {
                return Err(ExperimentalLiveContextRecoveryError::BindingCleanup {
                    binding,
                    cleanup: cleanup.to_string(),
                });
            }
            return Err(ExperimentalLiveContextRecoveryError::Authority(binding));
        }

        if let Err(binding) = authority
            .register_result_recovery_for_answer(recovery.clone())
            .await
        {
            authority
                .unbind_channel(&replacement_channel_id, recovery.session_id())
                .await;
            if let Err(cleanup) = self
                .orchestrator()
                .close_live_channel(
                    &self.host,
                    &replacement_channel_id,
                    Some(recovery.session_id()),
                )
                .await
            {
                return Err(ExperimentalLiveContextRecoveryError::BindingCleanup {
                    binding,
                    cleanup: cleanup.to_string(),
                });
            }
            return Err(ExperimentalLiveContextRecoveryError::Authority(binding));
        }
        if let Err(binding) = pending.bind_opened(&result).await {
            authority
                .unbind_channel(&replacement_channel_id, recovery.session_id())
                .await;
            let cleanup = self
                .orchestrator()
                .close_live_channel(
                    &self.host,
                    &replacement_channel_id,
                    Some(recovery.session_id()),
                )
                .await
                .map(|_| ())
                .map_err(|error| error.to_string());
            return Err(ExperimentalLiveContextRecoveryError::BindingCleanup {
                binding,
                cleanup: cleanup.err().unwrap_or_default(),
            });
        }
        Ok(ExperimentalLiveReplacementRequired::DelegationResult {
            open: result,
            canonical_seed_cursor: recovery.canonical_seed_cursor(),
        })
    }

    /// Retire a bound strict open that its caller could not publish.
    #[cfg(feature = "experimental-gpt-live")]
    pub async fn cleanup_execution_identity_publication_failure(
        &self,
        authority: &dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        session: &SessionId,
        channel: &LiveChannelId,
    ) -> Result<(), LiveChannelVerbError> {
        self.orchestrator()
            .cleanup_experimental_live_channel_after_publication_failure(
                &self.host, authority, session, channel,
            )
            .await
    }

    /// One close coordinator for ordinary and experimental channels. The
    /// exact experimental authority selects physical custody internally;
    /// surfaces never probe provider errors or retain an owner map.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn close_live_channel(
        &self,
        authority: Option<&dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider>,
        channel: &LiveChannelId,
    ) -> Result<LiveCloseStatus, ExperimentalLiveChannelCloseError> {
        let session = self
            .runtime_adapter
            .live_session_for_active_channel(channel)
            .await
            .ok_or(ExperimentalLiveChannelCloseError::BindingMismatch)?;
        let mut physical_bound = false;
        let mut physical_error = None;
        let lifecycle_lease = if let Some(authority) = authority {
            let lease = self
                .runtime_adapter
                .acquire_live_open_lifecycle_lease(&session)
                .await
                .map_err(|error| {
                    ExperimentalLiveChannelCloseError::LifecycleAuthority(error.to_string())
                })?;
            match authority.close_physical_if_bound(channel, &session).await {
                Ok(ExperimentalLivePhysicalClose::Closed) => physical_bound = true,
                Ok(ExperimentalLivePhysicalClose::NotBound) => {}
                Err(error) => {
                    // Physical failure must not prevent the generated semantic
                    // close. Exact provider custody is force-retired by
                    // `unbind_channel` only after that semantic close commits.
                    physical_bound = true;
                    physical_error = Some(error);
                }
            }
            Some(lease)
        } else {
            None
        };
        let result = self
            .orchestrator()
            .close_live_channel(&self.host, channel, Some(&session))
            .await?;
        self.runtime_adapter
            .retire_live_assistant_output_handles(&session, channel);
        if physical_bound && let Some(authority) = authority {
            authority.unbind_channel(channel, &session).await;
        }
        drop(lifecycle_lease);
        if let Some(error) = physical_error {
            Err(ExperimentalLiveChannelCloseError::PhysicalAuthority(error))
        } else {
            Ok(result.status)
        }
    }

    /// Strict close from an exact pending handle.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn close_experimental_live_pending_channel(
        &self,
        authority: &dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        channel_id: &LiveChannelId,
        pending_receipt: &str,
    ) -> Result<LiveCloseStatus, ExperimentalLiveChannelCloseError> {
        self.close_experimental_live_channel_with_receipt(
            authority,
            channel_id,
            &meerkat_runtime::meerkat_machine::LiveChannelCloseReceipt::Pending(
                pending_receipt.to_string(),
            ),
        )
        .await
    }

    /// Strict close from an exact active handle.
    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    pub async fn close_experimental_live_active_channel(
        &self,
        authority: &dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        channel_id: &LiveChannelId,
        activation_receipt: &str,
    ) -> Result<LiveCloseStatus, ExperimentalLiveChannelCloseError> {
        self.close_experimental_live_channel_with_receipt(
            authority,
            channel_id,
            &meerkat_runtime::meerkat_machine::LiveChannelCloseReceipt::Activation(
                activation_receipt.to_string(),
            ),
        )
        .await
    }

    #[cfg(all(feature = "live-webrtc", feature = "experimental-gpt-live"))]
    async fn close_experimental_live_channel_with_receipt(
        &self,
        authority: &dyn crate::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        channel_id: &LiveChannelId,
        receipt: &meerkat_runtime::meerkat_machine::LiveChannelCloseReceipt,
    ) -> Result<LiveCloseStatus, ExperimentalLiveChannelCloseError> {
        let session_id = self
            .experimental_live_session_for_channel(channel_id)
            .await
            .map_err(|error| {
                ExperimentalLiveChannelCloseError::LifecycleAuthority(error.to_string())
            })?;
        let current = match receipt {
            meerkat_runtime::meerkat_machine::LiveChannelCloseReceipt::Pending(value) => {
                self.runtime_adapter
                    .validate_live_channel_custody_by_pending_receipt(
                        &session_id,
                        channel_id,
                        value,
                    )
                    .await
            }
            meerkat_runtime::meerkat_machine::LiveChannelCloseReceipt::Activation(value) => {
                self.runtime_adapter
                    .validate_live_channel_custody_by_activation_receipt(
                        &session_id,
                        channel_id,
                        value,
                    )
                    .await
            }
        }
        .map_err(|error| {
            ExperimentalLiveChannelCloseError::LifecycleAuthority(error.to_string())
        })?;
        if matches!(
            current.state(),
            meerkat_runtime::meerkat_machine::LiveChannelCustodyState::Closed
        ) {
            return Ok(LiveCloseStatus::Closed);
        }
        let close_custody = self
            .runtime_adapter
            .revoke_live_channel_close_custody(&session_id, channel_id, receipt)
            .await
            .map_err(|error| {
                ExperimentalLiveChannelCloseError::LifecycleAuthority(error.to_string())
            })?;
        if close_custody.session_id() != &session_id || close_custody.channel_id() != channel_id {
            return Err(ExperimentalLiveChannelCloseError::LifecycleAuthority(
                "generated close custody did not match the exact channel binding".to_string(),
            ));
        }
        if close_custody.already_closed() {
            return Ok(LiveCloseStatus::Closed);
        }
        self.close_live_channel(Some(authority), channel_id).await
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> MemberLiveHost for ServiceMemberLiveHost<B> {
    async fn open(
        &self,
        session: &SessionId,
        turning_mode: Option<RealtimeTurningMode>,
        transport: Option<LiveOpenTransport>,
    ) -> Result<LiveOpenResult, MemberLiveError> {
        // ADJ-P6B-3: the member-side open runs under its OWN ceiling,
        // strictly inside the controlling bridge deadline, so the member
        // fails closed and replies typed BEFORE the caller times out. On
        // abort, any partially installed binding is evicted through the
        // shared fail-closed cleanup.
        let orchestrator = self.orchestrator();
        let opened = tokio::time::timeout(
            MEMBER_LIVE_OPEN_CEILING,
            orchestrator.open_live_channel(
                &self.host,
                self.transport_context(),
                Some(self.session_factory.as_ref()),
                session,
                turning_mode,
                transport,
            ),
        )
        .await;
        match opened {
            Ok(result) => result.map_err(member_live_error_from_open),
            Err(_elapsed) => {
                if let Some(channel_id) = self
                    .runtime_adapter
                    .live_active_channel_for_session(session)
                    .await
                {
                    self.orchestrator()
                        .close_live_channel_after_open_failure(&self.host, session, &channel_id)
                        .await;
                }
                Err(MemberLiveError::Unavailable {
                    reason: format!(
                        "live open exceeded the member ceiling of {}s and was aborted fail-closed",
                        MEMBER_LIVE_OPEN_CEILING.as_secs()
                    ),
                })
            }
        }
    }

    async fn close(
        &self,
        session: &SessionId,
        channel_id: &str,
    ) -> Result<LiveCloseStatus, MemberLiveError> {
        let channel = LiveChannelId::new(channel_id);
        #[cfg(feature = "live-webrtc")]
        if let Some(webrtc_state) = self.webrtc_state.as_ref() {
            webrtc_state
                .close_peer_checked(&channel)
                .await
                .map_err(|error| MemberLiveError::Unavailable {
                    reason: format!(
                        "WebRTC physical cleanup failed for live channel '{channel_id}': {error}"
                    ),
                })?;
        }
        self.orchestrator()
            .close_live_channel(&self.host, &channel, Some(session))
            .await
            .map(|result| result.status)
            .map_err(member_live_error_from_verb)
    }

    async fn status(
        &self,
        session: &SessionId,
        channel_id: Option<String>,
    ) -> Result<MemberLiveStatus, MemberLiveError> {
        // ADJ-P6B-2: absent channel ⇒ resolve the member's active channel
        // (the reply-loss discovery primitive); none active ⇒ typed
        // ChannelNotFound — an honest "nothing to reconcile".
        let channel = match channel_id {
            Some(channel_id) => LiveChannelId::new(channel_id),
            None => self
                .runtime_adapter
                .live_active_channel_for_session(session)
                .await
                .ok_or(MemberLiveError::ChannelNotFound)?,
        };
        let status = self
            .orchestrator()
            .live_channel_status(&self.host, &channel, Some(session))
            .await
            .map_err(member_live_error_from_verb)?;
        Ok(MemberLiveStatus {
            channel_id: channel.to_string(),
            status,
        })
    }

    async fn control(
        &self,
        session: &SessionId,
        channel_id: &str,
        verb: BridgeLiveControlVerb,
    ) -> Result<BridgeLiveControlOutcome, MemberLiveError> {
        let channel = LiveChannelId::new(channel_id);
        self.orchestrator()
            .control_live_channel(
                &self.host,
                self.transport_context(),
                &channel,
                Some(session),
                verb,
            )
            .await
            .map_err(member_live_error_from_verb)
    }
}

/// Exhaustive `LiveOpenError → MemberLiveError` projection (the §16.6
/// cause table's member column; DEC-P6B-L3/L4). NO wildcard arm: a new
/// pipeline variant forces this surface to decide.
#[must_use]
pub fn member_live_error_from_open(error: LiveOpenError) -> MemberLiveError {
    use crate::session_runtime::errors::LiveOpenPrecheckError;

    match error {
        LiveOpenError::SessionNotFound { .. } => MemberLiveError::Unavailable {
            reason: error.to_string(),
        },
        // DEC-P6B-L4 (§16.6 erratum): #302 fires BEFORE the provider is
        // resolved (S2 < S4) — fabricating a provider string for
        // `LiveAdapterUnavailable` would be folklore. On the member host
        // the state is composition-impossible anyway; the typed truth is
        // "no live transport".
        LiveOpenError::RealtimeFactoryMissing => MemberLiveError::TransportUnavailable,
        LiveOpenError::NoTransportConfigured | LiveOpenError::WebsocketNotConfigured => {
            MemberLiveError::TransportUnavailable
        }
        LiveOpenError::AdmissionRejectedAlreadyBound { .. } => MemberLiveError::ChannelAlreadyBound,
        LiveOpenError::AdmissionRejectedLifecycleClosed => MemberLiveError::Unavailable {
            reason: "session lifecycle is closed to live channel admission".to_string(),
        },
        LiveOpenError::Precheck(LiveOpenPrecheckError::ModelNotRealtime { model, provider }) => {
            MemberLiveError::ModelNotRealtime {
                model,
                provider: provider.to_string(),
            }
        }
        LiveOpenError::Precheck(LiveOpenPrecheckError::ProviderHasNoLiveAdapter { provider }) => {
            MemberLiveError::AdapterUnavailable {
                provider: provider.to_string(),
            }
        }
        LiveOpenError::ProviderUnsupportedByFactory { provider } => {
            MemberLiveError::AdapterUnavailable {
                provider: provider.to_string(),
            }
        }
        LiveOpenError::WebrtcNotConfigured | LiveOpenError::WebrtcNotCompiled => {
            MemberLiveError::TransportUnsupported {
                requested: "webrtc".to_string(),
            }
        }
        LiveOpenError::UnsupportedTransport => MemberLiveError::TransportUnsupported {
            requested: "unknown".to_string(),
        },
        error @ (LiveOpenError::SessionStateFault(_)
        | LiveOpenError::OpenConfig(_)
        | LiveOpenError::AdmissionAuthority(_)
        | LiveOpenError::AdmissionRejectedChannelCollision { .. }
        | LiveOpenError::AdmissionRejectedRevokedChannel { .. }
        | LiveOpenError::AdmissionRejectedNoReason
        | LiveOpenError::MissingHostHandoff
        | LiveOpenError::HostOpenSessionAlreadyBound { .. }
        | LiveOpenError::HostOpen(_)
        | LiveOpenError::Precheck(LiveOpenPrecheckError::SessionLookup { .. })
        | LiveOpenError::AdapterOpen(_)
        | LiveOpenError::AdapterAttach(_)
        | LiveOpenError::Ingress(_)
        | LiveOpenError::TokenMint(_)
        | LiveOpenError::AudioPolicyMissing
        | LiveOpenError::AudioFormatUnmappable { .. }
        | LiveOpenError::WebrtcClock(_)
        | LiveOpenError::WebrtcTokenMint(_)) => MemberLiveError::Internal {
            reason: error.to_string(),
        },
    }
}

/// Exhaustive `LiveChannelVerbError → MemberLiveError` projection
/// (DEC-P6B-L3 §3 verb table). Unbound channels and pin mismatches are the
/// typed `ChannelNotFound` (a channel of another member is unaddressable
/// through this member's lane); host not-found/not-ready classes degrade
/// `Unavailable`; everything else is `Internal`.
#[must_use]
pub fn member_live_error_from_verb(error: LiveChannelVerbError) -> MemberLiveError {
    use meerkat_runtime::meerkat_machine::dsl::{
        LiveChannelRequestRejectionReason, LiveCommandRejectionReason,
    };

    match error {
        LiveChannelVerbError::UnboundCommand { .. }
        | LiveChannelVerbError::UnboundRequest { .. }
        | LiveChannelVerbError::SessionPinMismatch { .. } => MemberLiveError::ChannelNotFound,
        LiveChannelVerbError::CommandRejected {
            authority, detail, ..
        } => match authority.rejection {
            LiveCommandRejectionReason::ChannelNotFound
            | LiveCommandRejectionReason::NoAdapter
            | LiveCommandRejectionReason::ChannelNotReady => {
                MemberLiveError::Unavailable { reason: detail }
            }
            LiveCommandRejectionReason::UnsupportedCommand
            | LiveCommandRejectionReason::AdapterError
            | LiveCommandRejectionReason::InternalHostError => {
                MemberLiveError::Internal { reason: detail }
            }
        },
        LiveChannelVerbError::RequestRejected {
            authority, detail, ..
        } => {
            let reason = detail.unwrap_or_else(|| "live channel request rejected".to_string());
            match authority.rejection {
                LiveChannelRequestRejectionReason::ChannelNotFound
                | LiveChannelRequestRejectionReason::NoAdapter => {
                    MemberLiveError::Unavailable { reason }
                }
                LiveChannelRequestRejectionReason::InvalidToken
                | LiveChannelRequestRejectionReason::InvalidPayload
                | LiveChannelRequestRejectionReason::WebrtcAnswerError
                | LiveChannelRequestRejectionReason::InternalHostError => {
                    MemberLiveError::Internal { reason }
                }
            }
        }
        error @ (LiveChannelVerbError::RejectionAuthorityFailed { .. }
        | LiveChannelVerbError::ResultAuthority { .. }
        | LiveChannelVerbError::CommitOmitted
        | LiveChannelVerbError::HostCommit { .. }
        | LiveChannelVerbError::ResultProjection { .. }
        | LiveChannelVerbError::RefreshConfig(_)) => MemberLiveError::Internal {
            reason: error.to_string(),
        },
    }
}

/// Live tool dispatch over the canonical service seam (DEC-P6B-L11): the
/// member host installs this on its `LiveAdapterHost` so live tool calls
/// raised mid-turn dispatch into the owning session's dispatcher —
/// identical semantics to `rkat-rpc`'s `RuntimeLiveToolDispatcher` (two
/// one-line adapters over ONE service method, no dual semantics).
pub struct ServiceLiveToolDispatcher {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
}

impl ServiceLiveToolDispatcher {
    #[must_use]
    pub fn new(service: Arc<PersistentSessionService<FactoryAgentBuilder>>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl meerkat_live::LiveToolDispatcher for ServiceLiveToolDispatcher {
    async fn dispatch_live_tool_call(
        &self,
        session_id: &SessionId,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_live::LiveToolDispatchError> {
        self.service
            .dispatch_external_tool_call_with_timeout_policy(
                session_id,
                call,
                meerkat_core::ToolDispatchTimeoutPolicy::Disabled,
            )
            .await
            .map_err(|err| meerkat_live::LiveToolDispatchError::from_session_error(session_id, err))
    }
}
