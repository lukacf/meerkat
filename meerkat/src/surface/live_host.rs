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
use meerkat_core::connection::RealmId;
use meerkat_core::types::SessionId;
use meerkat_live::{LiveAdapterHost, LiveChannelId, LiveWsState};
use meerkat_llm_core::realtime_session::RealtimeSessionFactory;
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::member_live::{
    MEMBER_LIVE_OPEN_CEILING, MemberLiveError, MemberLiveHost, MemberLiveStatus,
};

use crate::service_factory::FactoryAgentBuilder;
use crate::session_runtime::admission::StagedCapacityAdmissions;
use crate::session_runtime::errors::{LiveChannelVerbError, LiveIngressError, LiveOpenError};
use crate::session_runtime::live_orchestration::{
    LiveOrchestrator, LiveSessionIngressReconciler, LiveTransportContext,
};
use crate::session_runtime::runtime_state::ArchiveRuntimeCleanup;
use crate::{PersistentSessionService, StagedSessionRegistry};

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
pub struct ServiceMemberLiveHostConfig {
    /// The daemon's runtime-backed session service.
    pub service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
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
/// it per trait call from its `Arc`ed composition).
pub struct ServiceMemberLiveHost {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    staged_sessions: Arc<StagedSessionRegistry>,
    staged_capacity_admissions: StagedCapacityAdmissions,
    runtime_adapter: Arc<MeerkatMachine>,
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

impl ServiceMemberLiveHost {
    #[must_use]
    pub fn new(config: ServiceMemberLiveHostConfig) -> Self {
        Self {
            service: config.service,
            staged_sessions: Arc::new(StagedSessionRegistry::new()),
            staged_capacity_admissions: Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            runtime_adapter: config.runtime_adapter,
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

    fn orchestrator(&self) -> LiveOrchestrator<'_> {
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

    fn transport_context(&self) -> LiveTransportContext<'_> {
        // The member host never composes WebRTC (DEC-P6B-L8): `new` leaves
        // that optional transport absent, so `transport=webrtc` degrades
        // typed exactly as the non-compiled arm.
        LiveTransportContext::new(self.ws_state.as_deref(), self.base_url.as_deref())
    }
}

#[async_trait]
impl MemberLiveHost for ServiceMemberLiveHost {
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
