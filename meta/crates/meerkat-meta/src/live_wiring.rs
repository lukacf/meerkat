//! Live (realtime) member sessions through the meta gateways.
//!
//! Meta retains target parsing, access enforcement, transport packaging,
//! SDK-facing verbs, and its callback tool dispatcher. Both ordinary and
//! OpenAI Live projection use Meerkat's `ServiceLiveProjection`; OpenAI Live
//! open and channel lifecycle enter `ServiceMemberLiveHost`. Meta may
//! serialize a typed per-open identity/seed request, but it does not copy
//! projection, transcript assembly, token authority, or playback settlement.
//! The ordinary websocket open and handler paths retain their existing
//! transport behavior.
//!
//! Feature gating: the strict GPT Live machinery here (WebRTC answer, strict
//! pending/active custody, capability projection, public observation) is
//! compiled under `openai-live`, which serves the released public
//! `gpt-live-1` path via [`LiveCapabilityProvider::public`]. The deprecated
//! private ChatGPT-brokered path adds only its factory identity under
//! `experimental-gpt-live` via [`LiveCapabilityProvider::experimental`].

use std::sync::Arc;
#[cfg(feature = "openai-live")]
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use meerkat::AgentFactory;
#[cfg(feature = "openai-live")]
use meerkat::ExperimentalLiveFactoryIdentity;
#[cfg(feature = "openai-live")]
use meerkat::session_runtime::errors::LiveOpenError;
use meerkat::session_runtime::errors::LiveOpenPrecheckError;
#[cfg(not(feature = "openai-live"))]
use meerkat::session_runtime::live_orchestration::precheck_identity;
#[cfg(not(feature = "openai-live"))]
use meerkat::session_runtime::live_orchestration::realtime_projection_messages;
#[cfg(feature = "openai-live")]
use meerkat::session_runtime::live_orchestration::{
    LiveSeedWindow, RealtimeSessionOpenProjectionError,
};
use meerkat::session_runtime::realtime_credentials::RealtimeCurrentConfigSource;
use meerkat::surface::{ServiceLiveProjection, ServiceMemberLiveHost, ServiceMemberLiveHostConfig};
use meerkat_client::realtime_session::{RealtimeSessionFactory, RealtimeSessionOpenConfig};
#[cfg(feature = "openai-live")]
use meerkat_contracts::LivePlaybackCompleteParams;
use meerkat_contracts::LiveTruncateParams;
#[cfg(feature = "openai-live")]
use meerkat_contracts::{BridgeLiveControlVerb, LiveInputChunkWire, WireLiveResponseModality};
use meerkat_contracts::{
    LiveChannelParams, LiveCloseResult, LiveCommitInputParams, LiveCommitInputResult,
    LiveInterruptResult, LiveOpenTransport, LiveRefreshResult, LiveSendInputParams,
    LiveSendInputResult, LiveStatusResult, LiveTruncateResult, RealtimeTurningMode,
    WireLiveAdapterStatus, WireLiveDegradationReason,
};
#[cfg(not(feature = "openai-live"))]
use meerkat_contracts::{LiveOpenResult, RealtimeCapabilities};
#[cfg(feature = "openai-live")]
use meerkat_core::RealmId;
use meerkat_core::live_adapter::{LiveAdapterCommand, LiveProjectionSnapshot};
#[cfg(not(feature = "openai-live"))]
use meerkat_core::live_adapter::{
    LiveAudioConfig, LiveChannelCapabilities, LiveContinuityMode, LiveTransportBootstrap,
};
use meerkat_core::types::SessionId;
use meerkat_core::{Config, ConfigError, Provider, SessionLlmIdentity};
#[cfg(not(feature = "openai-live"))]
use meerkat_live::LiveChannelCloseObservation;
use meerkat_live::{
    LiveAdapterHost, LiveAdapterHostError, LiveChannelCloseFeedback, LiveChannelId,
    LiveChannelStatusFeedback, LiveProjectionSink, LiveToolDispatcher, LiveWsState,
    LiveWsTokenAuthority, live_input_chunk_from_wire,
};
use meerkat_runtime::MeerkatMachine;
use meerkat_session::{PersistentSessionService, SessionAgentBuilder};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[cfg(feature = "openai-live")]
use crate::access::{ACTION_AGENT_SEND, AccessView};
use crate::rpc::{JSONRPC_VERSION, JsonRpcError, JsonRpcResponse};

/// JSON-RPC error code for live methods invoked on a gateway with no live
/// context (ephemeral mode, or live disabled at init). The CALLER owns this
/// arm — `handle_live_method` is only reachable with a live context — but
/// the code and the `data.kind` discriminator live here so every surface
/// answers identically.
pub const LIVE_UNAVAILABLE_CODE: i64 = -32050;

/// `data.kind` discriminator carried by [`live_unavailable_response`].
pub const LIVE_UNAVAILABLE_KIND: &str = "live_unavailable";

const INVALID_PARAMS_CODE: i64 = -32602;
const METHOD_NOT_FOUND_CODE: i64 = -32601;
const INTERNAL_ERROR_CODE: i64 = -32000;

/// One live voice path per gateway, arbitrated "latest engaged wins" between
/// console voice and the external live channel.
pub mod owner;
pub use owner::{
    LiveOwner, LiveOwnerArbiter, LiveOwnerArbiterError, LiveOwnerCloser, LiveOwnerLease,
    LiveOwnerNotifier, LiveSupersededReason,
};

/// Which door a `LiveRpcHandler` serves. The external door arbitrates the
/// gateway's voice path at the RPC layer; the console door does not, because
/// the console controller engages the arbiter itself before dispatching.
#[cfg(feature = "openai-live")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveDoor {
    Console,
    External,
}

/// Operation-specific authority input for a future authenticated HTTP live
/// surface. Implementations must prove target ABAC, authoritative channel
/// ownership for channel verbs, and exact binding-use authorization for open
/// before returning success. Provider/credential materialization occurs only
/// after this seam returns `Ok(())`.
#[async_trait]
pub(crate) trait AuthenticatedHttpLiveAuthority: Send + Sync {
    fn principal(&self) -> &meerkat_core::PrincipalRef;

    async fn authorize(
        &self,
        operation: LiveOperation,
        resolved_session: Option<&SessionId>,
        params: &Value,
        machine: &MeerkatMachine,
    ) -> Result<(), JsonRpcError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveOperation {
    Open,
    #[cfg(feature = "openai-live")]
    ReplacementRequired,
    #[cfg(feature = "openai-live")]
    PlaybackOwnerRegister,
    #[cfg(feature = "openai-live")]
    PlaybackOwnerRevoke,
    Status,
    Close,
    Refresh,
    SendInput,
    CommitInput,
    Interrupt,
    Truncate,
    #[cfg(feature = "openai-live")]
    PlaybackComplete,
    #[cfg(feature = "openai-live")]
    WebrtcAnswer,
}

impl LiveOperation {
    fn from_method(method: &str) -> Option<Self> {
        match method {
            "meta/live/open" => Some(Self::Open),
            #[cfg(feature = "openai-live")]
            "meta/live/replacement_required" => Some(Self::ReplacementRequired),
            #[cfg(feature = "openai-live")]
            "meta/live/playback_owner/register" => Some(Self::PlaybackOwnerRegister),
            #[cfg(feature = "openai-live")]
            "meta/live/playback_owner/revoke" => Some(Self::PlaybackOwnerRevoke),
            "meta/live/status" => Some(Self::Status),
            "meta/live/close" => Some(Self::Close),
            "meta/live/refresh" => Some(Self::Refresh),
            "meta/live/send_input" => Some(Self::SendInput),
            "meta/live/commit_input" => Some(Self::CommitInput),
            "meta/live/interrupt" => Some(Self::Interrupt),
            "meta/live/truncate" => Some(Self::Truncate),
            #[cfg(feature = "openai-live")]
            "meta/live/playback_complete" => Some(Self::PlaybackComplete),
            #[cfg(feature = "openai-live")]
            meerkat_live::LIVE_WEBRTC_ANSWER_METHOD => Some(Self::WebrtcAnswer),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct LiveSurfaceAuthority {
    kind: LiveSurfaceAuthorityKind,
}

#[derive(Clone)]
enum LiveSurfaceAuthorityKind {
    HostTrustedStdio,
    AuthenticatedHttp(Arc<dyn AuthenticatedHttpLiveAuthority>),
}

impl LiveSurfaceAuthority {
    /// Explicit host authority for the local stdio control plane. Missing
    /// principals never select this mode implicitly.
    #[must_use]
    pub fn host_trusted_stdio() -> Self {
        Self {
            kind: LiveSurfaceAuthorityKind::HostTrustedStdio,
        }
    }

    /// Future HTTP composition seam. It is crate-private so an HTTP route
    /// cannot manufacture authority from an optional/missing principal; it
    /// must first construct the typed ABAC/owner/binding-use witness.
    #[allow(dead_code)]
    pub(crate) fn authenticated_http(witness: Arc<dyn AuthenticatedHttpLiveAuthority>) -> Self {
        Self {
            kind: LiveSurfaceAuthorityKind::AuthenticatedHttp(witness),
        }
    }

    async fn authorize(
        &self,
        operation: LiveOperation,
        resolved_session: Option<&SessionId>,
        params: &Value,
        machine: &MeerkatMachine,
    ) -> Result<(), JsonRpcError> {
        match &self.kind {
            LiveSurfaceAuthorityKind::HostTrustedStdio => Ok(()),
            LiveSurfaceAuthorityKind::AuthenticatedHttp(witness) => {
                let _principal = witness.principal();
                witness
                    .authorize(operation, resolved_session, params, machine)
                    .await
            }
        }
    }
}

#[cfg(feature = "openai-live")]
#[async_trait]
trait ExactLiveSessionOwner: Send + Sync {
    async fn owns_session(&self, canonical_session_id: &SessionId) -> bool;

    async fn validate_live_durable_source_availability(
        &self,
        canonical_session_id: &SessionId,
    ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError>;

    /// Readiness-only durable-source observation: never enters the mob actor
    /// queue and never loads the session body.
    async fn observe_live_durable_source_readiness(
        &self,
        canonical_session_id: &SessionId,
    ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError>;
}

#[cfg(feature = "openai-live")]
struct MobHandleLiveSessionOwner {
    handle: meerkat_mob::MobHandle,
    member_identity: meerkat_mob::AgentIdentity,
}

#[cfg(feature = "openai-live")]
#[async_trait]
impl ExactLiveSessionOwner for MobHandleLiveSessionOwner {
    async fn owns_session(&self, canonical_session_id: &SessionId) -> bool {
        self.handle
            .resolve_bridge_session_id(&self.member_identity)
            .await
            .as_ref()
            == Some(canonical_session_id)
    }

    async fn validate_live_durable_source_availability(
        &self,
        canonical_session_id: &SessionId,
    ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError> {
        use meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError;

        if !self.owns_session(canonical_session_id).await {
            return Err(ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable);
        }
        let member = self
            .handle
            .member(&self.member_identity)
            .await
            .map_err(|_| ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable)?;
        member
            .validate_live_durable_source_availability()
            .await
            .map_err(|_| ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable)?;
        if !self.owns_session(canonical_session_id).await {
            return Err(ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable);
        }
        Ok(())
    }

    async fn observe_live_durable_source_readiness(
        &self,
        canonical_session_id: &SessionId,
    ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError> {
        use meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError;
        use meerkat_mob::LiveDurableSourceReadinessError;

        // The handle checks the member's live binding against the published
        // machine snapshot and asks the session service for a body-free
        // observation of the durable source. No actor command, no body load.
        self.handle
            .observe_live_durable_source_readiness(&self.member_identity, canonical_session_id)
            .await
            .map_err(|error| {
                tracing::debug!(
                    member = %self.member_identity,
                    %canonical_session_id,
                    ?error,
                    "live durable source not ready"
                );
                match error {
                    LiveDurableSourceReadinessError::AccessDenied => {
                        ExperimentalLiveOpenAuthorityError::AccessDenied
                    }
                    _ => ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable,
                }
            })
    }
}

/// Authenticated Meta guard for one exact experimental live target.
///
/// The current member-session binding is re-read from MobMachine authority on
/// every use. Only after that binding and the request's immutable
/// [`AccessView`] both authorize `agent.send` does this guard delegate to the
/// Meerkat realm credential-policy authority that can mint the opaque binding
/// witness. Missing principals, stale sessions, and ABAC denials therefore
/// cannot reach credential materialization.
#[cfg(feature = "openai-live")]
pub struct MetaExperimentalLiveSessionBindingAuthority {
    owner: Arc<dyn ExactLiveSessionOwner>,
    machine: Arc<MeerkatMachine>,
    durable_identity: String,
    access_view: AccessView,
    credential_policy: Arc<dyn MetaExperimentalLiveBindingUsePolicy>,
}

/// Realm policy that mints only the exact opaque binding-use witness.
/// Meta attaches the generated AuthMachine lease from the same machine that
/// owns the revalidated member session.
#[cfg(feature = "openai-live")]
#[async_trait]
pub trait MetaExperimentalLiveBindingUsePolicy: Send + Sync {
    async fn authorize_binding_use(
        &self,
        canonical_session_id: &SessionId,
        selected_binding: &meerkat_core::AuthBindingRef,
    ) -> Result<
        meerkat_core::AuthBindingUseWitness,
        meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
    >;
}

#[cfg(feature = "openai-live")]
impl MetaExperimentalLiveSessionBindingAuthority {
    /// Bind a request-scoped access snapshot to the exact current member
    /// whose bridge session was selected by Meta's authoritative target
    /// resolver.
    #[must_use]
    pub fn new(
        handle: meerkat_mob::MobHandle,
        machine: Arc<MeerkatMachine>,
        member_identity: meerkat_mob::AgentIdentity,
        access_view: AccessView,
        credential_policy: Arc<dyn MetaExperimentalLiveBindingUsePolicy>,
    ) -> Self {
        let durable_identity =
            crate::member_comms_id::logical_memory_identity(member_identity.as_str());
        Self {
            owner: Arc::new(MobHandleLiveSessionOwner {
                handle,
                member_identity,
            }),
            machine,
            durable_identity,
            access_view,
            credential_policy,
        }
    }
}

#[cfg(feature = "openai-live")]
#[async_trait]
impl meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthority
    for MetaExperimentalLiveSessionBindingAuthority
{
    async fn validate_live_durable_source_availability(
        &self,
        canonical_session_id: &SessionId,
    ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError> {
        self.owner
            .validate_live_durable_source_availability(canonical_session_id)
            .await
    }

    async fn observe_live_durable_source_readiness(
        &self,
        canonical_session_id: &SessionId,
    ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError> {
        self.owner
            .observe_live_durable_source_readiness(canonical_session_id)
            .await
    }

    async fn authorize_binding_use(
        &self,
        canonical_session_id: &SessionId,
        selected_binding: &meerkat_core::AuthBindingRef,
    ) -> Result<
        meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthorization,
        meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
    > {
        use meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError;

        if !self.owner.owns_session(canonical_session_id).await {
            return Err(ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable);
        }
        if self.access_view.subject().is_none()
            || !self
                .access_view
                .allows_agent(ACTION_AGENT_SEND, &self.durable_identity)
        {
            return Err(ExperimentalLiveOpenAuthorityError::AccessDenied);
        }
        let binding_use = self
            .credential_policy
            .authorize_binding_use(canonical_session_id, selected_binding)
            .await?;
        Ok(
            meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthorization::from_machine_authority(
                binding_use,
                self.machine.generated_auth_lease_handle(),
            ),
        )
    }
}

/// Routes live tool calls into the member session's normal external-tool
/// dispatch. Experimental GPT Live admission separately rejects callback
/// provenance before this dispatcher can receive a live tool call.
/// Port of meerkat-rpc's `RuntimeLiveToolDispatcher`.
pub struct GatewayLiveToolDispatcher<B: SessionAgentBuilder + 'static> {
    service: Arc<PersistentSessionService<B>>,
}

impl<B: SessionAgentBuilder + 'static> GatewayLiveToolDispatcher<B> {
    pub fn new(service: Arc<PersistentSessionService<B>>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> LiveToolDispatcher for GatewayLiveToolDispatcher<B> {
    async fn dispatch_live_tool_call(
        &self,
        session_id: &SessionId,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_live::LiveToolDispatchError> {
        self.service
            .dispatch_external_tool_call(session_id, call)
            .await
            .map_err(|err| meerkat_live::LiveToolDispatchError::from_session_error(session_id, err))
    }
}

/// [`RealtimeCurrentConfigSource`] serving the gateway's effective `Config`.
///
/// meta gateways have no durable config store; per-open realtime
/// credential resolution rides the session identity's auth binding or the
/// provider default (env `OPENAI_API_KEY`) against this fixed config —
/// matching text-model behaviour. Embedders with real config stores can swap
/// in `StoreBackedRealtimeConfigSource` later.
pub struct EnvRealtimeConfigSource {
    config: Config,
}

impl EnvRealtimeConfigSource {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// The exact current-Config snapshot this source answers with.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }
}

#[async_trait]
impl RealtimeCurrentConfigSource for EnvRealtimeConfigSource {
    async fn current_config(&self) -> Result<Config, ConfigError> {
        Ok(self.config.clone())
    }
}

#[cfg(feature = "openai-live")]
#[async_trait]
impl meerkat::experimental_gpt_live::ExperimentalLiveCurrentConfigSource
    for EnvRealtimeConfigSource
{
    async fn current_config(&self) -> Result<Config, ConfigError> {
        Ok(self.config.clone())
    }
}

/// Everything a gateway needs to serve live channels: the adapter host, the
/// WS transport state (mount via `meerkat_live::live_ws_router(ws_state)` on
/// the gateway's HTTP app), the per-open realtime session factory, and the
/// externally-reachable base URL the WS bootstrap embeds.
#[derive(Clone)]
pub struct GatewayLiveContext {
    pub host: Arc<LiveAdapterHost>,
    pub ws_state: Arc<LiveWsState>,
    #[cfg(feature = "openai-live")]
    pub webrtc_state: Arc<meerkat_live::LiveWebrtcState>,
    /// The exact current-Config snapshot owner shared by ordinary realtime
    /// credential resolution and an opt-in experimental live authority.
    pub config_source: Arc<EnvRealtimeConfigSource>,
    pub session_factory: Arc<dyn RealtimeSessionFactory>,
    pub ws_base_url: String,
    /// Gateway-wide seed-projection clamp (`runtime_options.live.seed_max_chars`),
    /// overridable per open. `None` = no clamp. Stopgap for upstream ask 30
    /// (docs/design/upstream-asks.md): the provider caps live instructions at
    /// 65,536 tokens and long member transcripts overflow the projected seed.
    pub seed_max_chars: Option<usize>,
    /// The single live voice path this context serves. Console voice and the
    /// external `meta/live/*` door both engage it; the newest engagement
    /// wins and the loser is closed with a typed reason.
    pub arbiter: Arc<LiveOwnerArbiter>,
}

impl GatewayLiveContext {
    /// Project the same immutable current-Config owner into Meerkat's
    /// experimental admission seam without coupling the host to the public
    /// OpenAI realtime credential trait.
    #[cfg(feature = "openai-live")]
    #[must_use]
    pub fn experimental_live_config_source(
        &self,
    ) -> Arc<dyn meerkat::experimental_gpt_live::ExperimentalLiveCurrentConfigSource> {
        Arc::clone(&self.config_source)
            as Arc<dyn meerkat::experimental_gpt_live::ExperimentalLiveCurrentConfigSource>
    }
}

/// Compose the live stack for a persistent-mode gateway.
///
/// One shared Meerkat [`ServiceLiveProjection`] instance serves all four
/// injected trait seams (projection sink, close feedback, status feedback,
/// and WS token authority). The tool dispatcher rides the host builder with
/// the upstream default tool timeout
/// (`meerkat_live::DEFAULT_LIVE_TOOL_TIMEOUT`). Credentials resolve PER OPEN
/// via the facade's `PerOpenCredentialRealtimeSessionFactory` over
/// [`EnvRealtimeConfigSource`].
pub fn attach_live<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    machine: Arc<MeerkatMachine>,
    factory: &AgentFactory,
    config: Config,
    ws_base_url: String,
    seed_max_chars: Option<usize>,
) -> GatewayLiveContext {
    let projection = Arc::new(ServiceLiveProjection::new(
        Arc::clone(&service),
        Arc::clone(&machine),
    ));
    let dispatcher: Arc<dyn LiveToolDispatcher> = Arc::new(GatewayLiveToolDispatcher::new(service));
    let host = Arc::new(
        LiveAdapterHost::new(Arc::clone(&projection) as Arc<dyn LiveProjectionSink>)
            .with_live_tool_dispatcher(dispatcher),
    );
    let ws_state = Arc::new(LiveWsState::new(
        Arc::clone(&host),
        Arc::clone(&projection) as Arc<dyn LiveChannelCloseFeedback>,
        Arc::clone(&projection) as Arc<dyn LiveChannelStatusFeedback>,
        Arc::clone(&projection) as Arc<dyn LiveWsTokenAuthority>,
    ));
    #[cfg(feature = "openai-live")]
    let webrtc_state = Arc::new(meerkat_live::LiveWebrtcState::new(
        Arc::clone(&host),
        Arc::clone(&projection) as Arc<dyn LiveChannelCloseFeedback>,
        projection as Arc<dyn LiveChannelStatusFeedback>,
    ));
    let config_source = Arc::new(EnvRealtimeConfigSource::new(config));
    let session_factory = factory.build_openai_realtime_session_factory(
        Arc::clone(&config_source) as Arc<dyn RealtimeCurrentConfigSource>
    );
    GatewayLiveContext {
        host,
        ws_state,
        #[cfg(feature = "openai-live")]
        webrtc_state,
        config_source,
        session_factory,
        ws_base_url,
        seed_max_chars,
        arbiter: Arc::new(live_owner_arbiter_for(&machine)),
    }
}

/// The voice-path arbiter with its channel liveness bound to `machine`, so a
/// holder whose channel ended outside both doors is released, never trusted.
fn live_owner_arbiter_for(machine: &Arc<MeerkatMachine>) -> LiveOwnerArbiter {
    let arbiter = LiveOwnerArbiter::default();
    let machine = Arc::clone(machine);
    arbiter.set_liveness(Arc::new(move |channel: String| {
        let machine = Arc::clone(&machine);
        Box::pin(async move {
            machine
                .live_session_for_active_channel(&LiveChannelId::new(&channel))
                .await
                .is_some()
        })
    }));
    arbiter
}

// ---------------------------------------------------------------------------
// meta/live/* handlers. Open, WebRTC answer, and strict close are thin
// callers of Meerkat's shared facade coordinators. The remaining ordinary
// channel verbs retain their existing Meta surface projection.
// ---------------------------------------------------------------------------

/// `instructions` on an ordinary `meta/live/open` accepts a single string
/// or an array of strings. This is the legacy per-open overlay and is never
/// accepted by the strict experimental execution-identity branch.
#[derive(Deserialize)]
#[serde(untagged)]
enum LiveOpenInstructions {
    One(String),
    Many(Vec<String>),
}

impl LiveOpenInstructions {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(text) => vec![text],
            Self::Many(items) => items,
        }
    }
}

/// The gateway-side params for `meta/live/open`. The member target
/// (`identity` / `member_id` / `session_id`) is resolved by the CALLER
/// before `handle_live_method`; unknown fields are ignored here so the
/// target spellings pass through untouched.
#[derive(Deserialize)]
struct GatewayLiveOpenParams {
    #[serde(default)]
    turning_mode: Option<RealtimeTurningMode>,
    #[serde(default)]
    transport: Option<LiveOpenTransport>,
    #[cfg(feature = "openai-live")]
    #[serde(default)]
    execution_identity: Option<meerkat_contracts::WireLiveExecutionIdentityOverrideV1>,
    /// v1 realtime-model override (design §6): members whose text model is
    /// not realtime-capable open the channel against this model instead.
    #[serde(default)]
    model: Option<String>,
    /// Strict optional per-open provider selection (downstream app cross-provider
    /// regression): pairs with `model` so a member whose text profile lives
    /// on another provider (e.g. Anthropic) can open the channel against a
    /// provider that has a realtime lane (`provider = "openai"`,
    /// `model = "gpt-realtime-2"`). Parsed against the typed provider
    /// vocabulary via [`Provider::parse_strict`] - an unrecognized name is
    /// a typed invalid-params error, never a silent fallthrough to the
    /// inherited provider. Requires `model` (the pair is mutated together);
    /// when the selection differs from the inherited provider the inherited
    /// provider-specific auth binding is cleared so the selected provider's
    /// configured default credential resolution applies.
    #[serde(default)]
    provider: Option<String>,
    /// Legacy ordinary-live per-open instruction overlay. Strict
    /// experimental execution rejects this raw prose and accepts only the
    /// host-registered `execution_identity.profile_id` catalog selection.
    #[serde(default)]
    instructions: Option<LiveOpenInstructions>,
    /// Per-open override of the gateway-wide seed clamp
    /// (`runtime_options.live.seed_max_chars`).
    #[serde(default)]
    seed_max_chars: Option<usize>,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayLiveWebrtcAnswerParams {
    #[serde(default)]
    identity: Option<String>,
    channel_id: String,
    #[serde(default)]
    pending_receipt: Option<String>,
    #[serde(default)]
    readiness_receipt: Option<String>,
    token: String,
    offer_sdp: String,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLiveReceiptParams {
    identity: String,
    channel_id: String,
    #[serde(default)]
    pending_receipt: Option<String>,
    #[serde(default)]
    activation_receipt: Option<String>,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLivePlaybackOwnerRegisterParams {
    identity: String,
    channel_id: String,
    pending_receipt: String,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLivePlaybackOwnerRevokeParams {
    identity: String,
    channel_id: String,
    pending_receipt: String,
    readiness_receipt: String,
    #[serde(default)]
    activation_receipt: Option<String>,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLiveSendInputParams {
    identity: String,
    channel_id: String,
    activation_receipt: String,
    chunk: LiveInputChunkWire,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLiveCommitInputParams {
    identity: String,
    channel_id: String,
    activation_receipt: String,
    #[serde(default)]
    response_modality: Option<WireLiveResponseModality>,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLiveActiveChannelParams {
    identity: String,
    channel_id: String,
    activation_receipt: String,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLiveTruncateParams {
    identity: String,
    channel_id: String,
    activation_receipt: String,
    output_id: String,
    audio_played_ms: u64,
    #[serde(default)]
    reported_playback_prefix: Option<String>,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLivePlaybackCompleteParams {
    identity: String,
    channel_id: String,
    activation_receipt: String,
    output_id: String,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayLiveReplacementRequiredParams {
    #[serde(default)]
    identity: Option<String>,
    #[serde(default)]
    member_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default)]
    activation_receipt: Option<String>,
}

fn live_success(rpc_id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: rpc_id,
        result: Some(result),
        error: None,
    }
}

fn live_error(rpc_id: Value, code: i64, message: impl Into<String>) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: rpc_id,
        result: None,
        error: Some(JsonRpcError::new(code, message)),
    }
}

/// A live open refused because another live open on this gateway is still
/// being prepared: Meerkat admits one live open projection per process at a
/// time and queues none. Nothing was opened, and a new open succeeds once
/// that open is prepared, so callers treat it as busy, not as a host failure.
#[cfg(feature = "openai-live")]
pub(crate) const LIVE_OPEN_BUSY_CODE: i64 = -32054;

/// The `data.kind` of a [`LIVE_OPEN_BUSY_CODE`] refusal, which also carries
/// `data.retryable = true`.
#[cfg(feature = "openai-live")]
pub(crate) const LIVE_OPEN_BUSY_KIND: &str = "live_open_busy";

#[cfg(feature = "openai-live")]
fn live_open_error_response(rpc_id: Value, error: LiveOpenError) -> JsonRpcResponse {
    let code = match &error {
        LiveOpenError::OpenConfig(RealtimeSessionOpenProjectionError::Backpressured) => {
            LIVE_OPEN_BUSY_CODE
        }
        LiveOpenError::SessionNotFound { .. }
        | LiveOpenError::OpenConfig(RealtimeSessionOpenProjectionError::Session(
            meerkat_core::SessionError::NotFound { .. },
        ))
        | LiveOpenError::AdmissionRejectedAlreadyBound { .. }
        | LiveOpenError::AdmissionRejectedRevokedChannel { .. }
        | LiveOpenError::AdmissionRejectedLifecycleClosed
        | LiveOpenError::Precheck(LiveOpenPrecheckError::ModelNotRealtime { .. }) => {
            INVALID_PARAMS_CODE
        }
        LiveOpenError::SessionStateFault(_)
        | LiveOpenError::RealtimeFactoryMissing
        | LiveOpenError::OpenConfig(_)
        | LiveOpenError::AdmissionAuthority(_)
        | LiveOpenError::AdmissionRejectedChannelCollision { .. }
        | LiveOpenError::AdmissionRejectedNoReason
        | LiveOpenError::ExecutionProfileAdmission(_)
        | LiveOpenError::MissingHostHandoff
        | LiveOpenError::HostOpenSessionAlreadyBound { .. }
        | LiveOpenError::HostOpen(_)
        | LiveOpenError::Precheck(_)
        | LiveOpenError::ProviderUnsupportedByFactory { .. }
        | LiveOpenError::AdapterOpen(_)
        | LiveOpenError::AdapterAttach(_)
        | LiveOpenError::Ingress(_)
        | LiveOpenError::NoTransportConfigured
        | LiveOpenError::WebsocketNotConfigured
        | LiveOpenError::TokenMint(_)
        | LiveOpenError::AudioPolicyMissing
        | LiveOpenError::AudioFormatUnmappable { .. }
        | LiveOpenError::WebrtcNotConfigured
        | LiveOpenError::WebrtcNotCompiled
        | LiveOpenError::WebrtcClock(_)
        | LiveOpenError::WebrtcTokenMint(_)
        | LiveOpenError::UnsupportedTransport => INTERNAL_ERROR_CODE,
    };
    if code == LIVE_OPEN_BUSY_CODE {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: rpc_id,
            result: None,
            error: Some(
                JsonRpcError::new(code, error.to_string()).with_data(
                    serde_json::json!({ "kind": LIVE_OPEN_BUSY_KIND, "retryable": true }),
                ),
            ),
        };
    }
    live_error(rpc_id, code, error.to_string())
}

/// The CALLER answers this when the gateway has no [`GatewayLiveContext`]
/// (ephemeral mode, or `runtime_options.live` off). Kept here so every
/// surface emits the identical `-32050` / `data.kind = "live_unavailable"`
/// shape.
/// Type-erased live RPC entry point. The gateway — which knows the concrete
/// session-builder type `B` — captures its `GatewayLiveContext`, service, and
/// machine into this closure so the shared (non-generic) RPC dispatch in
/// `crate::rpc` can serve `meta/live/*` without a type parameter.
type LiveRpcDispatch = Arc<
    dyn Fn(
            LiveSurfaceAuthority,
            Option<meerkat_core::types::SessionId>,
            Option<String>,
            String,
            Value,
            Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = JsonRpcResponse> + Send>>
        + Send
        + Sync,
>;

#[cfg(feature = "openai-live")]
tokio::task_local! {
    static LIVE_RPC_RESPONSE_DELIVERY: Arc<StdMutex<Option<LiveRpcResponseDeliveryCustody>>>;
}

#[cfg(feature = "openai-live")]
#[async_trait]
trait LiveOpenPublicationCleanup: Send + Sync {
    async fn cleanup(&self) -> Result<(), String>;
}

#[cfg(feature = "openai-live")]
struct LiveOpenPublicationCleanupOwner<B: SessionAgentBuilder + 'static> {
    host: Arc<ServiceMemberLiveHost<B>>,
    authority: Arc<dyn meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider>,
    session: SessionId,
    channel: LiveChannelId,
}

#[cfg(feature = "openai-live")]
#[async_trait]
impl<B: SessionAgentBuilder + 'static> LiveOpenPublicationCleanup
    for LiveOpenPublicationCleanupOwner<B>
{
    async fn cleanup(&self) -> Result<(), String> {
        self.host
            .cleanup_execution_identity_publication_failure(
                self.authority.as_ref(),
                &self.session,
                &self.channel,
            )
            .await
            .map_err(|error| error.to_string())
    }
}

#[cfg(feature = "openai-live")]
pub(crate) struct LiveOpenResponseDeliveryCustody {
    cleanup: Option<Arc<dyn LiveOpenPublicationCleanup>>,
}

#[cfg(feature = "openai-live")]
impl LiveOpenResponseDeliveryCustody {
    fn new(cleanup: Arc<dyn LiveOpenPublicationCleanup>) -> Self {
        Self {
            cleanup: Some(cleanup),
        }
    }

    async fn delivered(mut self) -> Result<(), String> {
        self.cleanup.take();
        Ok(())
    }

    async fn rejected(mut self) -> Result<(), String> {
        let Some(cleanup) = self.cleanup.take() else {
            return Ok(());
        };
        cleanup.cleanup().await
    }
}

#[cfg(feature = "openai-live")]
impl Drop for LiveOpenResponseDeliveryCustody {
    fn drop(&mut self) {
        let Some(cleanup) = self.cleanup.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = cleanup.cleanup().await;
            });
        }
    }
}

#[cfg(feature = "openai-live")]
pub(crate) enum LiveRpcResponseDeliveryCustody {
    WebrtcAnswer(meerkat::surface::LiveWebrtcAnswerDeliveryCustody),
    Open(LiveOpenResponseDeliveryCustody),
}

#[cfg(feature = "openai-live")]
impl LiveRpcResponseDeliveryCustody {
    pub(crate) async fn delivered(self) -> Result<(), String> {
        match self {
            Self::WebrtcAnswer(custody) => {
                custody.delivered().await.map_err(|error| error.to_string())
            }
            Self::Open(custody) => custody.delivered().await,
        }
    }

    pub(crate) async fn rejected(self) -> Result<(), String> {
        match self {
            Self::WebrtcAnswer(custody) => {
                custody.rejected().await.map_err(|error| error.to_string())
            }
            Self::Open(custody) => custody.rejected().await,
        }
    }
}

/// Run one RPC dispatch with request-local custody for an accepted WebRTC
/// answer or strict live open. The caller settles it only after the outer
/// transport has published (or failed to publish) the serialized response.
#[cfg(feature = "openai-live")]
pub(crate) async fn capture_live_rpc_response_delivery<F, T>(
    future: F,
) -> (T, Option<LiveRpcResponseDeliveryCustody>)
where
    F: std::future::Future<Output = T>,
{
    let slot = Arc::new(StdMutex::new(None));
    let output = LIVE_RPC_RESPONSE_DELIVERY
        .scope(Arc::clone(&slot), future)
        .await;
    let delivery = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    (output, delivery)
}

#[cfg(feature = "openai-live")]
fn retain_live_webrtc_answer_delivery(
    custody: meerkat::surface::LiveWebrtcAnswerDeliveryCustody,
) -> impl std::future::Future<Output = Result<(), String>> {
    retain_live_rpc_response_delivery(LiveRpcResponseDeliveryCustody::WebrtcAnswer(custody))
}

#[cfg(feature = "openai-live")]
fn retain_live_open_response_delivery(
    custody: LiveOpenResponseDeliveryCustody,
) -> impl std::future::Future<Output = Result<(), String>> {
    retain_live_rpc_response_delivery(LiveRpcResponseDeliveryCustody::Open(custody))
}

#[cfg(feature = "openai-live")]
async fn retain_live_rpc_response_delivery(
    custody: LiveRpcResponseDeliveryCustody,
) -> Result<(), String> {
    {
        let mut custody = Some(custody);
        let retained = LIVE_RPC_RESPONSE_DELIVERY.try_with(|slot| {
            let mut slot = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot.is_some() {
                false
            } else {
                *slot = custody.take();
                true
            }
        });
        if matches!(retained, Ok(true)) {
            Ok(())
        } else {
            if let Some(custody) = custody {
                custody.rejected().await?;
            }
            Err("experimental live response requires a delivery-aware outer RPC writer".to_string())
        }
    }
}

/// Side-effect-free capability projection owned by the same configured
/// Meerkat factory that will admit strict GPT Live opens.
///
/// Stock Meta composition uses [`Self::disabled`]. An embedding host may
/// opt in through [`Self::public`] (released `gpt-live-1`, `openai-live`) by
/// supplying its already-configured factory, exact realm, and the composed
/// open authority, or through the deprecated [`Self::experimental`] path,
/// which additionally pins the exact experimental factory identity. Every
/// capabilities request revalidates those predicates upstream, so stale
/// realm, binding, Gate0, operator, or factory state fails closed to an
/// empty capability list.
#[derive(Clone)]
pub struct LiveCapabilityProvider {
    #[cfg(feature = "openai-live")]
    configured: Option<Arc<ConfiguredLiveCapabilityProvider>>,
    #[cfg(not(feature = "openai-live"))]
    _disabled: (),
}

#[cfg(feature = "openai-live")]
struct ConfiguredLiveCapabilityProvider {
    factory: Arc<AgentFactory>,
    realm: RealmId,
    /// Exact experimental factory identity pinned by the deprecated private
    /// registration. The public `gpt-live-1` path has no factory identity:
    /// its admission is the catalog row plus the configured binding, both
    /// already sealed inside `open_authority`.
    experimental_factory: Option<ExperimentalLiveFactoryIdentity>,
    open_authority: Arc<dyn meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider>,
    answer_transport: Arc<dyn meerkat_live::LiveWebrtcAnswerTransport>,
    public_observation_publisher:
        Arc<dyn meerkat::experimental_gpt_live::ExperimentalLivePublicObservationPublisher>,
    activator: ExperimentalLiveActivatorRegistration,
    live_adapter_host: Option<Arc<LiveAdapterHost>>,
    /// True only when the shared Meerkat pending/active authority contract is
    /// composed. The preexisting bound-ready binder is not sufficient.
    phase_authority_composed: bool,
}

#[cfg(feature = "openai-live")]
#[derive(Clone)]
enum ExperimentalLiveActivatorRegistration {
    Uncomposed(Arc<meerkat_mob_mcp::MobMcpState>),
    Composed(Arc<dyn meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator>),
}

impl Default for LiveCapabilityProvider {
    fn default() -> Self {
        Self::disabled()
    }
}

impl LiveCapabilityProvider {
    /// Fail-closed provider used by stock gateway composition.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            #[cfg(feature = "openai-live")]
            configured: None,
            #[cfg(not(feature = "openai-live"))]
            _disabled: (),
        }
    }

    /// Opt in to the public Live (`gpt-live-1`) path using the one atomic
    /// host registration required for a usable strict channel: the composed
    /// per-open authority (which seals the host-owned execution identity and
    /// configured OpenAI binding) and the sealed WebRTC answer transport.
    ///
    /// There is no operator, Gate0, or factory identity on this path. The
    /// advertised capability atoms come from
    /// `open_authority.execution_feature_capabilities()`.
    #[cfg(feature = "openai-live")]
    #[must_use]
    pub fn public(
        factory: Arc<AgentFactory>,
        realm: RealmId,
        open_authority: Arc<
            dyn meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        >,
        answer_transport: Arc<dyn meerkat_live::LiveWebrtcAnswerTransport>,
        mob_mcp_state: Arc<meerkat_mob_mcp::MobMcpState>,
        public_observation_publisher: Arc<
            dyn meerkat::experimental_gpt_live::ExperimentalLivePublicObservationPublisher,
        >,
    ) -> Self {
        Self::configured(
            factory,
            realm,
            None,
            open_authority,
            answer_transport,
            mob_mcp_state,
            public_observation_publisher,
        )
    }

    /// Opt in to the DEPRECATED private ChatGPT-brokered path using the one
    /// atomic host registration required for a usable experimental channel:
    /// qualification, per-open authority, and sealed WebRTC answer transport.
    #[cfg(feature = "experimental-gpt-live")]
    #[must_use]
    pub fn experimental(
        factory: Arc<AgentFactory>,
        realm: RealmId,
        experimental_factory: ExperimentalLiveFactoryIdentity,
        open_authority: Arc<
            dyn meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        >,
        answer_transport: Arc<dyn meerkat_live::LiveWebrtcAnswerTransport>,
        mob_mcp_state: Arc<meerkat_mob_mcp::MobMcpState>,
        public_observation_publisher: Arc<
            dyn meerkat::experimental_gpt_live::ExperimentalLivePublicObservationPublisher,
        >,
    ) -> Self {
        Self::configured(
            factory,
            realm,
            Some(experimental_factory),
            open_authority,
            answer_transport,
            mob_mcp_state,
            public_observation_publisher,
        )
    }

    #[cfg(feature = "openai-live")]
    fn configured(
        factory: Arc<AgentFactory>,
        realm: RealmId,
        experimental_factory: Option<ExperimentalLiveFactoryIdentity>,
        open_authority: Arc<
            dyn meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider,
        >,
        answer_transport: Arc<dyn meerkat_live::LiveWebrtcAnswerTransport>,
        mob_mcp_state: Arc<meerkat_mob_mcp::MobMcpState>,
        public_observation_publisher: Arc<
            dyn meerkat::experimental_gpt_live::ExperimentalLivePublicObservationPublisher,
        >,
    ) -> Self {
        Self {
            configured: Some(Arc::new(ConfiguredLiveCapabilityProvider {
                factory,
                realm,
                experimental_factory,
                open_authority,
                answer_transport,
                public_observation_publisher,
                activator: ExperimentalLiveActivatorRegistration::Uncomposed(mob_mcp_state),
                live_adapter_host: None,
                phase_authority_composed: false,
            })),
        }
    }

    #[cfg(feature = "openai-live")]
    fn compose_for_host<B: SessionAgentBuilder + 'static>(
        &self,
        machine: Arc<MeerkatMachine>,
        shared_live_host: Arc<ServiceMemberLiveHost<B>>,
        live_adapter_host: Arc<LiveAdapterHost>,
        execution_policy: meerkat_mob_mcp::live_delegation::LiveDelegationExecutionPolicy,
    ) -> Self {
        let Some(configured) = self.configured.as_ref() else {
            return Self::disabled();
        };
        let ExperimentalLiveActivatorRegistration::Uncomposed(mob_mcp_state) =
            &configured.activator
        else {
            return self.clone();
        };
        let downstream =
            meerkat_mob_mcp::live_delegation::compose_experimental_live_delegation_coordinator_with_policy(
                Arc::clone(&machine),
                Arc::clone(mob_mcp_state),
                execution_policy,
            );
        // Delegated work that outlived its voice channel: a reopened channel's
        // startup instructions say that earlier work is still finishing, so
        // the model does not claim it is done before its result arrives
        // (meerkat #1652). Meerkat's RPC router binds its coordinator the
        // same way; the first binding wins.
        configured
            .open_authority
            .bind_post_close_work_source(Arc::clone(&downstream)
                as Arc<dyn meerkat::experimental_gpt_live::LivePostCloseWorkSource>);
        let activator = meerkat::surface::ExperimentalGptLiveContextMirrorHost::new(
            machine,
            shared_live_host,
            Arc::clone(&configured.open_authority),
            downstream,
        );
        Self {
            configured: Some(Arc::new(ConfiguredLiveCapabilityProvider {
                factory: Arc::clone(&configured.factory),
                realm: configured.realm.clone(),
                experimental_factory: configured.experimental_factory.clone(),
                open_authority: Arc::clone(&configured.open_authority),
                answer_transport: Arc::clone(&configured.answer_transport),
                public_observation_publisher: Arc::clone(&configured.public_observation_publisher),
                activator: ExperimentalLiveActivatorRegistration::Composed(activator),
                live_adapter_host: Some(live_adapter_host),
                phase_authority_composed: true,
            })),
        }
    }

    /// Revalidate the upstream qualification and project only capabilities
    /// recognized by Meta's strict wire contract.
    #[must_use]
    pub fn feature_capabilities(&self) -> Vec<crate::live_contracts::FeatureCapability> {
        #[cfg(not(feature = "openai-live"))]
        {
            Vec::new()
        }
        #[cfg(feature = "openai-live")]
        {
            let Some(configured) = &self.configured else {
                return Vec::new();
            };
            if !configured.phase_authority_composed {
                return Vec::new();
            }
            let Some(live_adapter_host) = configured.live_adapter_host.as_ref() else {
                return Vec::new();
            };
            let ExperimentalLiveActivatorRegistration::Composed(bound_channel_activator) =
                &configured.activator
            else {
                return Vec::new();
            };
            if configured
                .open_authority
                .bound_ready_binder_for(
                    Arc::clone(bound_channel_activator),
                    Arc::clone(live_adapter_host),
                    Arc::clone(&configured.public_observation_publisher),
                )
                .is_none()
            {
                return Vec::new();
            }
            let Ok(capabilities) = configured.open_authority.execution_feature_capabilities()
            else {
                return Vec::new();
            };
            capabilities
                .iter()
                .filter(|capability| {
                    matches!(
                        **capability,
                        crate::live_contracts::LIVE_EXECUTION_IDENTITY_V1
                            | crate::live_contracts::LIVE_EXECUTION_FUNCTION_BRIDGE_V1
                            | crate::live_contracts::LIVE_EXECUTION_CLIENT_CONTEXT_V1
                    )
                })
                .filter_map(|capability| {
                    crate::live_contracts::FeatureCapability::new(*capability).ok()
                })
                .collect()
        }
    }

    #[cfg(feature = "openai-live")]
    fn open_authority(
        &self,
    ) -> Option<&dyn meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider> {
        self.configured
            .as_ref()
            .map(|configured| configured.open_authority.as_ref())
    }

    #[cfg(feature = "openai-live")]
    fn open_authority_arc(
        &self,
    ) -> Option<Arc<dyn meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider>>
    {
        self.configured
            .as_ref()
            .map(|configured| Arc::clone(&configured.open_authority))
    }

    #[cfg(feature = "openai-live")]
    fn answer_transport(&self) -> Option<&Arc<dyn meerkat_live::LiveWebrtcAnswerTransport>> {
        self.configured
            .as_ref()
            .map(|configured| &configured.answer_transport)
    }

    #[cfg(feature = "openai-live")]
    fn bound_ready_binder(&self) -> Option<Arc<dyn meerkat::surface::LiveWebrtcBoundReadyBinder>> {
        self.configured.as_ref().and_then(|configured| {
            let live_adapter_host = configured.live_adapter_host.as_ref()?;
            let ExperimentalLiveActivatorRegistration::Composed(bound_channel_activator) =
                &configured.activator
            else {
                return None;
            };
            configured.open_authority.bound_ready_binder_for(
                Arc::clone(bound_channel_activator),
                Arc::clone(live_adapter_host),
                Arc::clone(&configured.public_observation_publisher),
            )
        })
    }

    #[cfg(feature = "openai-live")]
    async fn pending_replacement_required(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat::surface::ExperimentalLiveReplacementRequired> {
        let configured = self.configured.as_ref()?;
        let ExperimentalLiveActivatorRegistration::Composed(activator) = &configured.activator
        else {
            return None;
        };
        activator.pending_replacement_required(session_id).await
    }
}

/// Internal receipt-custody read retained from the same shared live host.
#[cfg(feature = "openai-live")]
type LiveCustodyRead = Arc<
    dyn Fn(
            LiveChannelId,
            String,
        ) -> futures::future::BoxFuture<
            'static,
            Result<meerkat::surface::ExperimentalLiveChannelCustodyStatus, String>,
        > + Send
        + Sync,
>;

/// Type-erased live RPC registration plus its fail-closed capability owner.
#[derive(Clone)]
pub struct LiveRpcHandler {
    dispatch: LiveRpcDispatch,
    capability_provider: LiveCapabilityProvider,
    #[cfg(feature = "openai-live")]
    custody_read: LiveCustodyRead,
}

impl LiveRpcHandler {
    /// Console retains the original receipt; the caller supplies no custody.
    /// This does not alter the strict public Pending/Active status wire shape.
    #[cfg(feature = "openai-live")]
    pub(crate) async fn read_console_channel_custody(
        &self,
        session: &SessionId,
        channel: &LiveChannelId,
        pending_receipt: &str,
    ) -> Result<meerkat::surface::ExperimentalLiveChannelCustodyStatus, String> {
        let custody = (self.custody_read)(channel.clone(), pending_receipt.to_string()).await?;
        validate_strict_custody_target(&custody, session, channel).map_err(ToString::to_string)?;
        Ok(custody)
    }

    #[cfg(feature = "openai-live")]
    pub(crate) async fn pending_replacement(
        &self,
        session: &SessionId,
    ) -> Option<meerkat::surface::ExperimentalLiveReplacementRequired> {
        self.capability_provider
            .pending_replacement_required(session)
            .await
    }

    #[must_use]
    pub fn feature_capabilities(&self) -> Vec<crate::live_contracts::FeatureCapability> {
        self.capability_provider.feature_capabilities()
    }

    #[must_use]
    pub fn supports_live_execution_identity_v1(&self) -> bool {
        self.feature_capabilities().iter().any(|capability| {
            capability.as_str() == crate::live_contracts::LIVE_EXECUTION_IDENTITY_V1
        })
    }

    pub async fn dispatch(
        &self,
        authority: LiveSurfaceAuthority,
        resolved_session: Option<meerkat_core::types::SessionId>,
        canonical_target_identity: Option<String>,
        method: String,
        params: Value,
        rpc_id: Value,
    ) -> JsonRpcResponse {
        (self.dispatch)(
            authority,
            resolved_session,
            canonical_target_identity,
            method,
            params,
            rpc_id,
        )
        .await
    }

    /// Dispatch through the same response-delivery custody used by the outer
    /// RPC writer, then acknowledge successful publication. This is exposed
    /// only for offline realtime fixtures; it does not construct or replace
    /// live admission, binding-use, auth-lease, or provider authority.
    #[cfg(feature = "test-realtime-fixtures")]
    #[doc(hidden)]
    pub async fn dispatch_with_successful_delivery_for_test(
        &self,
        authority: LiveSurfaceAuthority,
        resolved_session: Option<meerkat_core::types::SessionId>,
        canonical_target_identity: Option<String>,
        method: String,
        params: Value,
        rpc_id: Value,
    ) -> Result<JsonRpcResponse, String> {
        let (response, delivery) = capture_live_rpc_response_delivery(self.dispatch(
            authority,
            resolved_session,
            canonical_target_identity,
            method,
            params,
            rpc_id,
        ))
        .await;
        if let Some(delivery) = delivery {
            delivery.delivered().await?;
        }
        Ok(response)
    }
}

/// Erase `handle_live_method` over the gateway's concrete types.
pub fn live_rpc_handler<B: SessionAgentBuilder + 'static>(
    ctx: Arc<GatewayLiveContext>,
    service: Arc<PersistentSessionService<B>>,
    machine: Arc<MeerkatMachine>,
) -> LiveRpcHandler {
    live_rpc_handler_with_capabilities(ctx, service, machine, LiveCapabilityProvider::disabled())
}

/// Erase `handle_live_method` with an explicit, revalidating capability
/// provider installed by the embedding application.
#[cfg(feature = "openai-live")]
pub fn live_rpc_handler_with_capabilities<B: SessionAgentBuilder + 'static>(
    ctx: Arc<GatewayLiveContext>,
    service: Arc<PersistentSessionService<B>>,
    machine: Arc<MeerkatMachine>,
    capability_provider: LiveCapabilityProvider,
) -> LiveRpcHandler {
    live_rpc_handler_with_policy(
        ctx,
        service,
        machine,
        capability_provider,
        None,
        meerkat_mob_mcp::live_delegation::LiveDelegationExecutionPolicy::default(),
        LiveDoor::External,
    )
}

#[cfg(feature = "openai-live")]
pub(crate) fn live_rpc_handler_with_console_policy<B: SessionAgentBuilder + 'static>(
    ctx: Arc<GatewayLiveContext>,
    service: Arc<PersistentSessionService<B>>,
    machine: Arc<MeerkatMachine>,
    capability_provider: LiveCapabilityProvider,
    summary: meerkat::session_runtime::live_summary::LiveContextSummaryPolicy,
) -> LiveRpcHandler {
    live_rpc_handler_with_policy(
        ctx,
        service,
        machine,
        capability_provider,
        Some(summary),
        meerkat_mob_mcp::live_delegation::LiveDelegationExecutionPolicy::ExistingMember,
        LiveDoor::Console,
    )
}

#[cfg(feature = "openai-live")]
fn live_rpc_handler_with_policy<B: SessionAgentBuilder + 'static>(
    ctx: Arc<GatewayLiveContext>,
    service: Arc<PersistentSessionService<B>>,
    machine: Arc<MeerkatMachine>,
    capability_provider: LiveCapabilityProvider,
    summary: Option<meerkat::session_runtime::live_summary::LiveContextSummaryPolicy>,
    execution_policy: meerkat_mob_mcp::live_delegation::LiveDelegationExecutionPolicy,
    door: LiveDoor,
) -> LiveRpcHandler {
    let host = shared_live_host(&ctx, &service, &machine);
    let host = match summary {
        Some(policy) => host.with_context_summary_policy(policy),
        None => host,
    };
    let shared_live_host = Arc::new(host);
    let custody_host = Arc::clone(&shared_live_host);
    let custody_read: LiveCustodyRead = Arc::new(move |channel, receipt| {
        let host = Arc::clone(&custody_host);
        Box::pin(async move {
            host.validate_experimental_live_channel_custody(&channel, &receipt)
                .await
                .map_err(|error| error.to_string())
        })
    });
    #[cfg(feature = "openai-live")]
    let capability_provider = capability_provider.compose_for_host(
        Arc::clone(&machine),
        Arc::clone(&shared_live_host),
        Arc::clone(&ctx.host),
        execution_policy,
    );
    let dispatch_capability_provider = capability_provider.clone();
    let dispatch =
        Arc::new(
            move |authority: LiveSurfaceAuthority,
                  resolved_session: Option<SessionId>,
                  canonical_target_identity: Option<String>,
                  method: String,
                  params: Value,
                  rpc_id: Value|
                  -> std::pin::Pin<
                Box<dyn std::future::Future<Output = JsonRpcResponse> + Send>,
            > {
                let ctx = Arc::clone(&ctx);
                let service = Arc::clone(&service);
                let machine = Arc::clone(&machine);
                let shared_live_host = Arc::clone(&shared_live_host);
                let capability_provider = dispatch_capability_provider.clone();
                Box::pin(async move {
                    handle_live_method_with_host(
                        &ctx,
                        &service,
                        &machine,
                        &shared_live_host,
                        &capability_provider,
                        door,
                        authority,
                        resolved_session,
                        canonical_target_identity,
                        &method,
                        &params,
                        rpc_id,
                    )
                    .await
                })
            },
        );
    LiveRpcHandler {
        dispatch,
        capability_provider,
        custody_read,
    }
}

/// Stock 0.8.26 registration keeps the preexisting ordinary live dispatcher
/// and projects no experimental capability.
#[cfg(not(feature = "openai-live"))]
pub fn live_rpc_handler_with_capabilities<B: SessionAgentBuilder + 'static>(
    ctx: Arc<GatewayLiveContext>,
    service: Arc<PersistentSessionService<B>>,
    machine: Arc<MeerkatMachine>,
    _capability_provider: LiveCapabilityProvider,
) -> LiveRpcHandler {
    let dispatch =
        Arc::new(
            move |authority: LiveSurfaceAuthority,
                  resolved_session: Option<SessionId>,
                  _canonical_target_identity: Option<String>,
                  method: String,
                  params: Value,
                  rpc_id: Value|
                  -> std::pin::Pin<
                Box<dyn std::future::Future<Output = JsonRpcResponse> + Send>,
            > {
                let ctx = Arc::clone(&ctx);
                let service = Arc::clone(&service);
                let machine = Arc::clone(&machine);
                Box::pin(async move {
                    handle_live_method(
                        &ctx,
                        &service,
                        &machine,
                        authority,
                        resolved_session,
                        &method,
                        &params,
                        rpc_id,
                    )
                    .await
                })
            },
        );
    LiveRpcHandler {
        dispatch,
        capability_provider: LiveCapabilityProvider::disabled(),
    }
}

/// Test-only: a member live host over `ctx`, so a test can end a channel the
/// way meerkat-live does when a WebSocket drops (without either RPC door).
#[cfg(all(test, feature = "openai-live"))]
pub(crate) fn member_live_host_for_test<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    service: &Arc<PersistentSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
) -> ServiceMemberLiveHost<B> {
    shared_live_host(ctx, service, machine)
}

// Ungated: the DEFAULT build now needs a `ServiceMemberLiveHost` too, to reach
// the stock truncate owner seam. The only experimental-specific line in the
// body already carries its own inner cfg.
fn shared_live_host<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    service: &Arc<PersistentSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
) -> ServiceMemberLiveHost<B> {
    let host = ServiceMemberLiveHost::new(ServiceMemberLiveHostConfig {
        service: Arc::clone(service),
        runtime_adapter: Arc::clone(machine),
        host: Arc::clone(&ctx.host),
        ws_state: Some(Arc::clone(&ctx.ws_state)),
        base_url: Some(ctx.ws_base_url.clone()),
        session_factory: Arc::clone(&ctx.session_factory),
        realm_id: None,
        instance_id: None,
        backend: None,
    });
    #[cfg(feature = "openai-live")]
    let host = host.with_webrtc_cleanup_state(Arc::clone(&ctx.webrtc_state));
    host
}

#[must_use]
pub fn live_unavailable_response(rpc_id: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: rpc_id,
        result: None,
        error: Some(
            JsonRpcError::new(
                LIVE_UNAVAILABLE_CODE,
                "live sessions are not available on this gateway",
            )
            .with_data(serde_json::json!({ "kind": LIVE_UNAVAILABLE_KIND })),
        ),
    }
}

fn parse_live_params<T: DeserializeOwned>(
    params: &Value,
    rpc_id: &Value,
) -> Result<T, Box<JsonRpcResponse>> {
    serde_json::from_value(params.clone()).map_err(|err| {
        Box::new(live_error(
            rpc_id.clone(),
            INVALID_PARAMS_CODE,
            format!("invalid params: {err}"),
        ))
    })
}

/// Dispatch a `meta/live/*` method against a live-enabled gateway.
///
/// `resolved_session` is the member target already canonicalized by the
/// caller (identity -> member alias -> raw session id - the same
/// canonicalization class as `/agents/{id}/events`); only `meta/live/open`
/// consumes it, every other method addresses a `channel_id`. `service` and
/// `machine` are passed alongside the (deliberately non-generic)
/// [`GatewayLiveContext`] because open/refresh must re-project the member
/// session and every handler resolves results through machine authority.
#[cfg(feature = "openai-live")]
#[allow(
    clippy::too_many_arguments,
    reason = "published live RPC boundary keeps each authority input explicit"
)]
pub async fn handle_live_method<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    service: &Arc<PersistentSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
    authority: LiveSurfaceAuthority,
    resolved_session: Option<SessionId>,
    method: &str,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let shared_live_host = Arc::new(shared_live_host(ctx, service, machine));
    let capability_provider = LiveCapabilityProvider::disabled();
    handle_live_method_with_host(
        ctx,
        service,
        machine,
        &shared_live_host,
        &capability_provider,
        LiveDoor::External,
        authority,
        resolved_session,
        None,
        method,
        params,
        rpc_id,
    )
    .await
}

/// Dispatch the published 0.8.26 ordinary live API without constructing the
/// newer generic Meerkat host facade. Surface authority remains enforced
/// before the original handler sequence runs.
#[cfg(not(feature = "openai-live"))]
#[allow(
    clippy::too_many_arguments,
    reason = "published live RPC boundary keeps each authority input explicit"
)]
pub async fn handle_live_method<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    service: &Arc<PersistentSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
    authority: LiveSurfaceAuthority,
    resolved_session: Option<SessionId>,
    method: &str,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let Some(operation) = LiveOperation::from_method(method) else {
        return live_error(
            rpc_id,
            METHOD_NOT_FOUND_CODE,
            format!("unknown live method {method}"),
        );
    };
    if let Err(error) = authority
        .authorize(operation, resolved_session.as_ref(), params, machine)
        .await
    {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: rpc_id,
            result: None,
            error: Some(error),
        };
    }
    match method {
        "meta/live/open" => {
            let Some(session_id) = resolved_session else {
                return live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    "live/open requires a resolvable member target \
                     (identity, member_id, or session_id)",
                );
            };
            handle_live_open(ctx, service, machine, &session_id, params, rpc_id).await
        }
        "meta/live/status" => handle_live_status(ctx, machine, params, rpc_id).await,
        "meta/live/close" => handle_live_close(ctx, machine, params, rpc_id).await,
        "meta/live/refresh" => handle_live_refresh(ctx, service, machine, params, rpc_id).await,
        "meta/live/send_input" => handle_live_send_input(ctx, machine, params, rpc_id).await,
        "meta/live/commit_input" => handle_live_commit_input(ctx, machine, params, rpc_id).await,
        "meta/live/interrupt" => handle_live_interrupt(ctx, machine, params, rpc_id).await,
        "meta/live/truncate" => handle_live_truncate(ctx, service, machine, params, rpc_id).await,
        _ => unreachable!("LiveOperation::from_method admitted only known methods"),
    }
}

#[cfg(feature = "openai-live")]
// A dispatch entry point legitimately carries the whole call context; splitting
// it into a struct would only move the arity behind a constructor.
#[allow(clippy::too_many_arguments)]
async fn handle_live_method_with_host<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    service: &Arc<PersistentSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
    shared_live_host: &Arc<ServiceMemberLiveHost<B>>,
    capability_provider: &LiveCapabilityProvider,
    door: LiveDoor,
    authority: LiveSurfaceAuthority,
    resolved_session: Option<SessionId>,
    canonical_target_identity: Option<String>,
    method: &str,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let Some(operation) = LiveOperation::from_method(method) else {
        return live_error(
            rpc_id,
            METHOD_NOT_FOUND_CODE,
            format!("unknown live method {method}"),
        );
    };
    if let Err(error) = authority
        .authorize(operation, resolved_session.as_ref(), params, machine)
        .await
    {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: rpc_id,
            result: None,
            error: Some(error),
        };
    }
    if let Some(response) = handle_strict_experimental_live_method(
        ctx,
        service,
        shared_live_host,
        capability_provider,
        resolved_session.as_ref(),
        canonical_target_identity.as_deref(),
        method,
        params,
        rpc_id.clone(),
    )
    .await
    {
        return response;
    }
    if let Some(response) = reject_missing_receipt_for_strict_channel(
        ctx,
        shared_live_host,
        machine,
        resolved_session.as_ref(),
        method,
        params,
        rpc_id.clone(),
    )
    .await
    {
        return response;
    }
    match method {
        "meta/live/open" => {
            let Some(session_id) = resolved_session else {
                return live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    "live/open requires a resolvable member target \
                     (identity, member_id, or session_id)",
                );
            };
            match door {
                // The console controller already engaged the voice path for
                // this call; arbitrating again here would make the console
                // supersede itself.
                LiveDoor::Console => {
                    handle_live_open(
                        ctx,
                        shared_live_host,
                        capability_provider,
                        &session_id,
                        canonical_target_identity,
                        params,
                        rpc_id,
                    )
                    .await
                }
                LiveDoor::External => {
                    handle_live_open_arbitrated(
                        ctx,
                        machine,
                        shared_live_host,
                        capability_provider,
                        &session_id,
                        canonical_target_identity,
                        params,
                        rpc_id,
                    )
                    .await
                }
            }
        }
        #[cfg(feature = "openai-live")]
        "meta/live/replacement_required" => {
            let Some(session_id) = resolved_session else {
                return live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    "live/replacement_required requires a resolvable durable member target",
                );
            };
            handle_live_replacement_required(
                capability_provider,
                &session_id,
                canonical_target_identity,
                params,
                rpc_id,
            )
            .await
        }
        "meta/live/status" => handle_live_status(ctx, machine, params, rpc_id).await,
        "meta/live/close" => {
            let response = handle_live_close(
                ctx,
                machine,
                shared_live_host,
                capability_provider,
                params,
                rpc_id,
            )
            .await;
            release_live_owner_after_close(ctx, params, response)
        }
        "meta/live/refresh" => handle_live_refresh(ctx, service, machine, params, rpc_id).await,
        "meta/live/send_input" => handle_live_send_input(ctx, machine, params, rpc_id).await,
        "meta/live/commit_input" => handle_live_commit_input(ctx, machine, params, rpc_id).await,
        "meta/live/interrupt" => handle_live_interrupt(ctx, machine, params, rpc_id).await,
        #[cfg(feature = "openai-live")]
        "meta/live/truncate" => handle_live_truncate(shared_live_host, params, rpc_id).await,
        #[cfg(feature = "openai-live")]
        "meta/live/playback_complete" => {
            handle_live_playback_complete(shared_live_host, params, rpc_id).await
        }
        #[cfg(feature = "openai-live")]
        meerkat_live::LIVE_WEBRTC_ANSWER_METHOD => {
            handle_live_webrtc_answer(shared_live_host, capability_provider, params, rpc_id).await
        }
        _ => unreachable!("LiveOperation::from_method admitted only known methods"),
    }
}

#[cfg(feature = "openai-live")]
fn local_live_execution_mode(
    mode: meerkat_core::LiveExecutionMode,
) -> crate::live_contracts::LiveExecutionMode {
    match mode {
        meerkat_core::LiveExecutionMode::FunctionBridge => {
            crate::live_contracts::LiveExecutionMode::FunctionBridge
        }
        meerkat_core::LiveExecutionMode::ClientContext => {
            crate::live_contracts::LiveExecutionMode::ClientContext
        }
    }
}

#[cfg(feature = "openai-live")]
fn validate_strict_custody_target(
    custody: &meerkat::surface::ExperimentalLiveChannelCustodyStatus,
    resolved_session: &SessionId,
    requested_channel: &LiveChannelId,
) -> Result<(), &'static str> {
    if custody.session_id() != resolved_session {
        return Err("live handle target no longer owns the channel session");
    }
    if custody.channel_id() != requested_channel {
        return Err("live handle channel does not match machine custody");
    }
    Ok(())
}

#[cfg(feature = "openai-live")]
fn strict_target<'a>(
    resolved_session: Option<&'a SessionId>,
    canonical_target_identity: Option<&'a str>,
) -> Result<(&'a SessionId, &'a str), &'static str> {
    match (resolved_session, canonical_target_identity) {
        (Some(session), Some(identity)) if !identity.is_empty() => Ok((session, identity)),
        _ => Err("strict live operation requires current durable identity authority"),
    }
}

#[cfg(feature = "openai-live")]
async fn strict_custody_by_activation<B: SessionAgentBuilder + 'static>(
    shared_live_host: &ServiceMemberLiveHost<B>,
    channel_id: &LiveChannelId,
    activation_receipt: &str,
    resolved_session: &SessionId,
) -> Result<meerkat::surface::ExperimentalLiveChannelCustodyStatus, String> {
    let custody = shared_live_host
        .validate_experimental_live_channel_custody_by_activation(channel_id, activation_receipt)
        .await
        .map_err(|error| error.to_string())?;
    validate_strict_custody_target(&custody, resolved_session, channel_id)
        .map_err(ToString::to_string)?;
    if !matches!(
        custody.phase(),
        meerkat::surface::ExperimentalLiveChannelPhaseStatus::Active { .. }
    ) {
        return Err("live active receipt no longer carries active authority".to_string());
    }
    Ok(custody)
}

#[cfg(feature = "openai-live")]
async fn reject_missing_receipt_for_strict_channel<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    shared_live_host: &ServiceMemberLiveHost<B>,
    machine: &MeerkatMachine,
    resolved_session: Option<&SessionId>,
    method: &str,
    params: &Value,
    rpc_id: Value,
) -> Option<JsonRpcResponse> {
    if method == "meta/live/open"
        || method == "meta/live/playback_owner/register"
        || method == "meta/live/playback_owner/revoke"
        || params.get("pending_receipt").is_some()
        || params.get("activation_receipt").is_some()
    {
        return None;
    }
    let channel_id = if let Some(channel_id) = params.get("channel_id").and_then(Value::as_str) {
        LiveChannelId::new(channel_id)
    } else {
        machine
            .live_active_channel_for_session(resolved_session?)
            .await?
    };
    // A channel the voice-path arbiter already closed carries no live
    // authority left to protect; its owner asking why it closed must get the
    // typed reason from status/close rather than a receipt demand.
    if matches!(method, "meta/live/status" | "meta/live/close")
        && ctx.arbiter.close_reason(channel_id.as_str()).is_some()
    {
        return None;
    }
    match shared_live_host
        .experimental_live_channel_phase(&channel_id)
        .await
    {
        Ok(Some(_)) => Some(live_error(
            rpc_id,
            INVALID_PARAMS_CODE,
            "strict live channel operation requires its exact phase receipt",
        )),
        Ok(None) | Err(_) => None,
    }
}

#[cfg(feature = "openai-live")]
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn handle_strict_experimental_live_method<B: SessionAgentBuilder + 'static>(
    _ctx: &GatewayLiveContext,
    _service: &Arc<PersistentSessionService<B>>,
    shared_live_host: &Arc<ServiceMemberLiveHost<B>>,
    capability_provider: &LiveCapabilityProvider,
    resolved_session: Option<&SessionId>,
    canonical_target_identity: Option<&str>,
    method: &str,
    params: &Value,
    rpc_id: Value,
) -> Option<JsonRpcResponse> {
    let strict = method == "meta/live/playback_owner/register"
        || method == "meta/live/playback_owner/revoke"
        || params.get("pending_receipt").is_some()
        || params.get("activation_receipt").is_some()
        || params.get("readiness_receipt").is_some();
    if !strict {
        return None;
    }
    let (resolved_session, canonical_target_identity) =
        match strict_target(resolved_session, canonical_target_identity) {
            Ok(target) => target,
            Err(error) => return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error)),
        };
    match method {
        "meta/live/playback_owner/register" => {
            let parsed: StrictLivePlaybackOwnerRegisterParams =
                match parse_live_params(params, &rpc_id) {
                    Ok(parsed) => parsed,
                    Err(response) => return Some(*response),
                };
            let _ = parsed.identity;
            let register_started = std::time::Instant::now();
            let channel_id = LiveChannelId::new(parsed.channel_id);
            let custody = match shared_live_host
                .validate_experimental_live_channel_custody(&channel_id, &parsed.pending_receipt)
                .await
            {
                Ok(custody) => custody,
                Err(error) => {
                    return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()));
                }
            };
            if let Err(error) =
                validate_strict_custody_target(&custody, resolved_session, &channel_id)
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            if !matches!(
                custody.phase(),
                meerkat::surface::ExperimentalLiveChannelPhaseStatus::Pending
            ) {
                return Some(live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    "playback owner registration requires pending channel authority",
                ));
            }
            let readiness = match shared_live_host
                .register_experimental_live_playback_owner(&channel_id, &parsed.pending_receipt)
                .await
            {
                Ok(readiness) => readiness,
                Err(error) => {
                    return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()));
                }
            };
            tracing::debug!(
                target: "meerkat_meta::console_voice::timing",
                %channel_id,
                elapsed_ms = u64::try_from(register_started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "live playback owner registered"
            );
            let result = crate::live_contracts::LivePlaybackOwnerReadiness {
                channel_id: readiness.channel_id().to_string(),
                readiness_receipt: readiness.readiness_receipt().to_string(),
            };
            Some(match serde_json::to_value(result) {
                Ok(value) => live_success(rpc_id, value),
                Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
            })
        }
        "meta/live/playback_owner/revoke" => {
            let parsed: StrictLivePlaybackOwnerRevokeParams =
                match parse_live_params(params, &rpc_id) {
                    Ok(parsed) => parsed,
                    Err(response) => return Some(*response),
                };
            let _ = parsed.identity;
            let channel_id = LiveChannelId::new(parsed.channel_id);
            let pending_custody = match shared_live_host
                .validate_experimental_live_channel_custody(&channel_id, &parsed.pending_receipt)
                .await
            {
                Ok(custody) => custody,
                Err(error) => {
                    return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()));
                }
            };
            if let Err(error) =
                validate_strict_custody_target(&pending_custody, resolved_session, &channel_id)
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            match (&parsed.activation_receipt, pending_custody.phase()) {
                (None, meerkat::surface::ExperimentalLiveChannelPhaseStatus::Pending) => {}
                (
                    Some(activation_receipt),
                    meerkat::surface::ExperimentalLiveChannelPhaseStatus::Active { .. },
                ) => {
                    if let Err(error) = strict_custody_by_activation(
                        shared_live_host,
                        &channel_id,
                        activation_receipt,
                        resolved_session,
                    )
                    .await
                    {
                        return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
                    }
                }
                (None, _) => {
                    return Some(live_error(
                        rpc_id,
                        INVALID_PARAMS_CODE,
                        "active playback-owner loss requires the exact activation receipt",
                    ));
                }
                (Some(_), _) => {
                    return Some(live_error(
                        rpc_id,
                        INVALID_PARAMS_CODE,
                        "activation receipt is valid only for an active playback owner",
                    ));
                }
            }
            if let Err(error) = shared_live_host
                .revoke_experimental_live_playback_owner(
                    &channel_id,
                    &parsed.pending_receipt,
                    &parsed.readiness_receipt,
                    parsed.activation_receipt.as_deref(),
                )
                .await
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()));
            }
            let revoked = match shared_live_host
                .validate_experimental_live_channel_custody(&channel_id, &parsed.pending_receipt)
                .await
            {
                Ok(custody) => custody,
                Err(error) => {
                    return Some(live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()));
                }
            };
            if let Err(error) =
                validate_strict_custody_target(&revoked, resolved_session, &channel_id)
            {
                return Some(live_error(rpc_id, INTERNAL_ERROR_CODE, error));
            }
            if !matches!(
                revoked.phase(),
                meerkat::surface::ExperimentalLiveChannelPhaseStatus::Revoked
            ) {
                return Some(live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    "playback-owner revoke did not project revoked custody",
                ));
            }
            Some(
                match serde_json::to_value(
                    crate::live_contracts::ExperimentalLiveChannelStatus::Revoked,
                ) {
                    Ok(value) => live_success(rpc_id, value),
                    Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
                },
            )
        }
        "meta/live/status" => {
            let parsed: StrictLiveReceiptParams = match parse_live_params(params, &rpc_id) {
                Ok(parsed) => parsed,
                Err(response) => return Some(*response),
            };
            let _ = parsed.identity;
            let channel_id = LiveChannelId::new(parsed.channel_id);
            let custody = match (parsed.pending_receipt, parsed.activation_receipt) {
                (Some(pending), None) => {
                    shared_live_host
                        .validate_experimental_live_channel_custody(&channel_id, &pending)
                        .await
                }
                (None, Some(active)) => {
                    shared_live_host
                        .validate_experimental_live_channel_custody_by_activation(
                            &channel_id,
                            &active,
                        )
                        .await
                }
                _ => {
                    return Some(live_error(
                        rpc_id,
                        INVALID_PARAMS_CODE,
                        "live/status requires exactly one phase receipt",
                    ));
                }
            };
            let custody = match custody {
                Ok(custody) => custody,
                Err(error) => {
                    return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()));
                }
            };
            if let Err(error) =
                validate_strict_custody_target(&custody, resolved_session, &channel_id)
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            let status = match custody.phase() {
                meerkat::surface::ExperimentalLiveChannelPhaseStatus::Pending => {
                    crate::live_contracts::ExperimentalLiveChannelStatus::Pending
                }
                meerkat::surface::ExperimentalLiveChannelPhaseStatus::Active {
                    activation_receipt,
                } => crate::live_contracts::ExperimentalLiveChannelStatus::Active {
                    handle: crate::live_contracts::ActiveLiveChannelHandle {
                        channel_id: custody.channel_id().to_string(),
                        target_identity: canonical_target_identity.to_string(),
                        execution_mode: local_live_execution_mode(custody.execution_mode()),
                        activation_receipt: activation_receipt.clone(),
                    },
                },
                meerkat::surface::ExperimentalLiveChannelPhaseStatus::Revoked => {
                    crate::live_contracts::ExperimentalLiveChannelStatus::Revoked
                }
                meerkat::surface::ExperimentalLiveChannelPhaseStatus::Closed => {
                    crate::live_contracts::ExperimentalLiveChannelStatus::Closed
                }
            };
            Some(match serde_json::to_value(status) {
                Ok(value) => live_success(rpc_id, value),
                Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
            })
        }
        "meta/live/close" => {
            let open_authority = match capability_provider.open_authority() {
                Some(authority) => authority,
                None => {
                    return Some(live_error(
                        rpc_id,
                        crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
                        "experimental live channel authority is not available",
                    ));
                }
            };
            let parsed: StrictLiveReceiptParams = match parse_live_params(params, &rpc_id) {
                Ok(parsed) => parsed,
                Err(response) => return Some(*response),
            };
            let _ = parsed.identity;
            let channel_id = LiveChannelId::new(parsed.channel_id);
            let status = match (parsed.pending_receipt, parsed.activation_receipt) {
                (Some(pending), None) => {
                    let custody = match shared_live_host
                        .validate_experimental_live_channel_custody(&channel_id, &pending)
                        .await
                    {
                        Ok(custody) => custody,
                        Err(error) => {
                            return Some(live_error(
                                rpc_id,
                                INVALID_PARAMS_CODE,
                                error.to_string(),
                            ));
                        }
                    };
                    if let Err(error) =
                        validate_strict_custody_target(&custody, resolved_session, &channel_id)
                    {
                        return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
                    }
                    shared_live_host
                        .close_experimental_live_pending_channel(
                            open_authority,
                            &channel_id,
                            &pending,
                        )
                        .await
                }
                (None, Some(active)) => {
                    let custody = match shared_live_host
                        .validate_experimental_live_channel_custody_by_activation(
                            &channel_id,
                            &active,
                        )
                        .await
                    {
                        Ok(custody) => custody,
                        Err(error) => {
                            return Some(live_error(
                                rpc_id,
                                INVALID_PARAMS_CODE,
                                error.to_string(),
                            ));
                        }
                    };
                    if let Err(error) =
                        validate_strict_custody_target(&custody, resolved_session, &channel_id)
                    {
                        return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
                    }
                    shared_live_host
                        .close_experimental_live_active_channel(
                            open_authority,
                            &channel_id,
                            &active,
                        )
                        .await
                }
                _ => {
                    return Some(live_error(
                        rpc_id,
                        INVALID_PARAMS_CODE,
                        "live/close requires exactly one phase receipt",
                    ));
                }
            };
            Some(match status {
                Ok(status) => match serde_json::to_value(LiveCloseResult { status }) {
                    Ok(value) => live_success(rpc_id, value),
                    Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
                },
                Err(error) => live_error(rpc_id, live_close_error_code(&error), error.to_string()),
            })
        }
        "meta/live/send_input" => {
            let parsed: StrictLiveSendInputParams = match parse_live_params(params, &rpc_id) {
                Ok(parsed) => parsed,
                Err(response) => return Some(*response),
            };
            let _ = parsed.identity;
            let channel_id = LiveChannelId::new(parsed.channel_id);
            if let Err(error) = strict_custody_by_activation(
                shared_live_host,
                &channel_id,
                &parsed.activation_receipt,
                resolved_session,
            )
            .await
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            let chunk = match live_input_chunk_from_wire(parsed.chunk) {
                Ok(chunk) => chunk,
                Err(error) => {
                    return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()));
                }
            };
            let result = shared_live_host
                .send_experimental_live_input(&channel_id, &parsed.activation_receipt, chunk)
                .await;
            Some(match result {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(value) => live_success(rpc_id, value),
                    Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
                },
                Err(error) => live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()),
            })
        }
        "meta/live/commit_input" => {
            let parsed: StrictLiveCommitInputParams = match parse_live_params(params, &rpc_id) {
                Ok(parsed) => parsed,
                Err(response) => return Some(*response),
            };
            let _ = parsed.identity;
            if parsed.response_modality.is_some() {
                return Some(live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    "strict live commit_input does not accept response_modality",
                ));
            }
            let channel_id = LiveChannelId::new(parsed.channel_id);
            if let Err(error) = strict_custody_by_activation(
                shared_live_host,
                &channel_id,
                &parsed.activation_receipt,
                resolved_session,
            )
            .await
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            let result = shared_live_host
                .control_experimental_live_channel(
                    &channel_id,
                    &parsed.activation_receipt,
                    BridgeLiveControlVerb::CommitInput,
                )
                .await;
            Some(strict_control_response(rpc_id, result))
        }
        "meta/live/interrupt" | "meta/live/refresh" => {
            let parsed: StrictLiveActiveChannelParams = match parse_live_params(params, &rpc_id) {
                Ok(parsed) => parsed,
                Err(response) => return Some(*response),
            };
            let _ = parsed.identity;
            let channel_id = LiveChannelId::new(parsed.channel_id);
            if let Err(error) = strict_custody_by_activation(
                shared_live_host,
                &channel_id,
                &parsed.activation_receipt,
                resolved_session,
            )
            .await
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            let verb = if method == "meta/live/interrupt" {
                BridgeLiveControlVerb::Interrupt
            } else {
                BridgeLiveControlVerb::Refresh
            };
            let result = shared_live_host
                .control_experimental_live_channel(&channel_id, &parsed.activation_receipt, verb)
                .await;
            Some(strict_control_response(rpc_id, result))
        }
        "meta/live/truncate" => {
            let parsed: StrictLiveTruncateParams = match parse_live_params(params, &rpc_id) {
                Ok(parsed) => parsed,
                Err(response) => return Some(*response),
            };
            let _ = parsed.identity;
            if parsed.output_id.trim().is_empty() {
                return Some(live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    "output_id must be non-empty",
                ));
            }
            let channel_id = LiveChannelId::new(parsed.channel_id);
            if let Err(error) = strict_custody_by_activation(
                shared_live_host,
                &channel_id,
                &parsed.activation_receipt,
                resolved_session,
            )
            .await
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            let result = shared_live_host
                .truncate_live_output(
                    &channel_id,
                    &parsed.activation_receipt,
                    &parsed.output_id,
                    parsed.audio_played_ms,
                    parsed.reported_playback_prefix,
                )
                .await;
            Some(match result {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(value) => live_success(rpc_id, value),
                    Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
                },
                Err(error) => live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()),
            })
        }
        "meta/live/playback_complete" => {
            let parsed: StrictLivePlaybackCompleteParams = match parse_live_params(params, &rpc_id)
            {
                Ok(parsed) => parsed,
                Err(response) => return Some(*response),
            };
            let _ = parsed.identity;
            if parsed.output_id.trim().is_empty() {
                return Some(live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    "output_id must be non-empty",
                ));
            }
            let channel_id = LiveChannelId::new(parsed.channel_id);
            if let Err(error) = strict_custody_by_activation(
                shared_live_host,
                &channel_id,
                &parsed.activation_receipt,
                resolved_session,
            )
            .await
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            let result = shared_live_host
                .complete_live_playback(&channel_id, &parsed.activation_receipt, &parsed.output_id)
                .await;
            Some(match result {
                Ok(result) => match serde_json::to_value(result) {
                    Ok(value) => live_success(rpc_id, value),
                    Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
                },
                Err(error) => live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()),
            })
        }
        meerkat_live::LIVE_WEBRTC_ANSWER_METHOD => {
            let parsed: GatewayLiveWebrtcAnswerParams = match parse_live_params(params, &rpc_id) {
                Ok(parsed) => parsed,
                Err(response) => return Some(*response),
            };
            let _ = parsed.identity;
            let (Some(pending_receipt), Some(readiness_receipt)) =
                (parsed.pending_receipt, parsed.readiness_receipt)
            else {
                return Some(live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    "strict WebRTC answer requires pending and readiness receipts",
                ));
            };
            let channel_id = LiveChannelId::new(parsed.channel_id);
            let custody = match shared_live_host
                .validate_experimental_live_channel_custody(&channel_id, &pending_receipt)
                .await
            {
                Ok(custody) => custody,
                Err(error) => {
                    return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()));
                }
            };
            if let Err(error) =
                validate_strict_custody_target(&custody, resolved_session, &channel_id)
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            if !matches!(
                custody.phase(),
                meerkat::surface::ExperimentalLiveChannelPhaseStatus::Pending
            ) {
                return Some(live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    "strict WebRTC answer requires pending channel authority",
                ));
            }
            let Some(answer_transport) = capability_provider.answer_transport() else {
                return Some(live_error(
                    rpc_id,
                    crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
                    "experimental live WebRTC answer transport is not available",
                ));
            };
            let Some(bound_ready_binder) = capability_provider.bound_ready_binder() else {
                return Some(live_error(
                    rpc_id,
                    crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
                    "experimental live WebRTC bound-ready authority is not available",
                ));
            };
            let coordinated = match shared_live_host
                .answer_experimental_live_webrtc_offer(
                    Arc::clone(answer_transport),
                    bound_ready_binder,
                    channel_id,
                    &pending_receipt,
                    &readiness_receipt,
                    parsed.token,
                    parsed.offer_sdp,
                )
                .await
            {
                Ok(coordinated) => coordinated,
                Err(error) => {
                    return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()));
                }
            };
            let value = match serde_json::to_value(meerkat_contracts::LiveWebrtcAnswerResult {
                answer_sdp: coordinated.answer_sdp,
            }) {
                Ok(value) => value,
                Err(error) => {
                    let cleanup_error = coordinated.delivery_custody.rejected().await.err();
                    return Some(live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        match cleanup_error {
                            Some(cleanup_error) => format!(
                                "failed to serialize LiveWebrtcAnswerResult: {error}; answer rejection failed: {cleanup_error}"
                            ),
                            None => format!("failed to serialize LiveWebrtcAnswerResult: {error}"),
                        },
                    ));
                }
            };
            Some(
                match retain_live_webrtc_answer_delivery(coordinated.delivery_custody).await {
                    Ok(()) => live_success(rpc_id, value),
                    Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error),
                },
            )
        }
        "meta/live/replacement_required" => {
            let parsed: StrictLiveActiveChannelParams = match parse_live_params(params, &rpc_id) {
                Ok(parsed) => parsed,
                Err(response) => return Some(*response),
            };
            let _ = parsed.identity;
            let channel_id = LiveChannelId::new(parsed.channel_id);
            if let Err(error) = strict_custody_by_activation(
                shared_live_host,
                &channel_id,
                &parsed.activation_receipt,
                resolved_session,
            )
            .await
            {
                return Some(live_error(rpc_id, INVALID_PARAMS_CODE, error));
            }
            Some(
                handle_live_replacement_required(
                    capability_provider,
                    resolved_session,
                    Some(canonical_target_identity.to_string()),
                    params,
                    rpc_id,
                )
                .await,
            )
        }
        _ => None,
    }
}

#[cfg(feature = "openai-live")]
fn strict_control_response(
    rpc_id: Value,
    result: Result<
        meerkat_contracts::BridgeLiveControlOutcome,
        meerkat::session_runtime::errors::LiveChannelVerbError,
    >,
) -> JsonRpcResponse {
    match result {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => live_success(rpc_id, value),
            Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
        },
        Err(error) => live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()),
    }
}

#[cfg(feature = "openai-live")]
async fn handle_live_replacement_required(
    capability_provider: &LiveCapabilityProvider,
    session_id: &SessionId,
    canonical_target_identity: Option<String>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: GatewayLiveReplacementRequiredParams = match parse_live_params(params, &rpc_id) {
        Ok(parsed) => parsed,
        Err(response) => return *response,
    };
    let _ = (
        parsed.identity,
        parsed.member_id,
        parsed.session_id,
        parsed.channel_id,
        parsed.activation_receipt,
    );
    let Some(target_identity) = canonical_target_identity else {
        return live_error(
            rpc_id,
            INVALID_PARAMS_CODE,
            "live/replacement_required requires canonical durable identity authority",
        );
    };
    if capability_provider.open_authority().is_none() {
        return live_error(
            rpc_id,
            crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
            format!(
                "capability {} is not available",
                crate::live_contracts::LIVE_EXECUTION_IDENTITY_V1
            ),
        );
    }
    let replacement = capability_provider
        .pending_replacement_required(session_id)
        .await;
    let result = match replacement {
        None => crate::live_contracts::LiveReplacementRequiredResult::not_required(),
        Some(meerkat::surface::ExperimentalLiveReplacementRequired::CanonicalContext {
            open,
            canonical_seed_cursor,
            ..
        }) => crate::live_contracts::LiveReplacementRequiredResult::required(
            crate::live_contracts::LiveReplacementReason::CanonicalContext,
            crate::live_contracts::LiveChannelHandle::from_open_result(target_identity, open),
            canonical_seed_cursor,
        ),
        Some(meerkat::surface::ExperimentalLiveReplacementRequired::DelegationResult {
            open,
            canonical_seed_cursor,
            ..
        }) => crate::live_contracts::LiveReplacementRequiredResult::required(
            crate::live_contracts::LiveReplacementReason::DelegationResult,
            crate::live_contracts::LiveChannelHandle::from_open_result(target_identity, open),
            canonical_seed_cursor,
        ),
    };
    match serde_json::to_value(result) {
        Ok(value) => live_success(rpc_id, value),
        Err(error) => live_error(
            rpc_id,
            INTERNAL_ERROR_CODE,
            format!("failed to serialize live replacement bootstrap: {error}"),
        ),
    }
}

#[cfg(feature = "openai-live")]
async fn handle_live_webrtc_answer<B: SessionAgentBuilder + 'static>(
    shared_live_host: &ServiceMemberLiveHost<B>,
    capability_provider: &LiveCapabilityProvider,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: GatewayLiveWebrtcAnswerParams = match parse_live_params(params, &rpc_id) {
        Ok(parsed) => parsed,
        Err(response) => return *response,
    };
    let Some(answer_transport) = capability_provider.answer_transport() else {
        return live_error(
            rpc_id,
            crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
            "experimental live WebRTC answer transport is not available",
        );
    };
    let Some(bound_ready_binder) = capability_provider.bound_ready_binder() else {
        return live_error(
            rpc_id,
            crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
            "experimental live WebRTC bound-ready authority is not available",
        );
    };
    let answer_started = std::time::Instant::now();
    let channel_id = parsed.channel_id.clone();
    let coordinated = match shared_live_host
        .answer_webrtc_offer(
            Arc::clone(answer_transport),
            Some(bound_ready_binder),
            LiveChannelId::new(parsed.channel_id),
            parsed.token,
            parsed.offer_sdp,
        )
        .await
    {
        Ok(coordinated) => coordinated,
        Err(error) => {
            return live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string());
        }
    };
    tracing::info!(
        target: "meerkat_meta::console_voice::timing",
        channel_id,
        elapsed_ms = u64::try_from(answer_started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "live webrtc answer negotiated with provider"
    );
    match serde_json::to_value(meerkat_contracts::LiveWebrtcAnswerResult {
        answer_sdp: coordinated.answer_sdp,
    }) {
        Ok(value) => match retain_live_webrtc_answer_delivery(coordinated.delivery_custody).await {
            Ok(()) => live_success(rpc_id, value),
            Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error),
        },
        Err(error) => {
            let cleanup_error = coordinated.delivery_custody.rejected().await.err();
            live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                match cleanup_error {
                    Some(cleanup_error) => format!(
                        "failed to serialize LiveWebrtcAnswerResult: {error}; answer rejection failed: {cleanup_error}"
                    ),
                    None => format!("failed to serialize LiveWebrtcAnswerResult: {error}"),
                },
            )
        }
    }
}

/// Normalize the legacy ordinary-live instruction overlay without promoting
/// it into durable transcript truth or the strict experimental profile lane.
fn live_open_instruction_overlay(instructions: Vec<String>) -> Option<String> {
    let joined = instructions
        .iter()
        .map(|instruction| instruction.trim())
        .filter(|instruction| !instruction.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!joined.is_empty()).then_some(joined)
}

/// Project the member session into a [`RealtimeSessionOpenConfig`], together
/// with whether a run-start hook refused the session's staged create-time
/// prompt at this open (`true`: the open result reports the
/// `staged_prompt_refused` advisory).
///
/// meta mirror of the facade `LiveOrchestrator::live_open_config_for_session`
/// (which is pinned to `FactoryAgentBuilder` + the staged-session registry,
/// so a generic-`B` gateway cannot borrow it): the same three published
/// service seams feed the same three pub projection free functions, and the
/// staged prompt is delivered at the same point, once the open-projection
/// admission is held and before the seed is read
/// ([`deliver_staged_prompt_for_live_open`]). No
/// staged/archived recovery - meta member sessions are held live by the
/// bridge, and an archived target surfaces as `NotFound`. Only the
/// non-`openai-live` open uses it; refresh never projects through open.
#[cfg(not(feature = "openai-live"))]
async fn live_open_config_for_session<B: SessionAgentBuilder + 'static>(
    service: &PersistentSessionService<B>,
    session_id: &SessionId,
    turning_mode: RealtimeTurningMode,
    seed_max_chars: Option<usize>,
) -> Result<(RealtimeSessionOpenConfig, bool), meerkat_core::SessionError> {
    // Mirror of the facade orchestrator: process-wide open-projection custody
    // is acquired BEFORE the persistent service can hydrate blob-backed image
    // history; the take-once slot on the returned config carries this same
    // lease through provider seed acknowledgement.
    let open_projection_lease = meerkat_core::RealtimeOpenProjectionAdmission::global()
        .try_acquire()
        .map_err(|error| {
            meerkat_core::SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                error.to_string(),
            ))
        })?;
    // A staged create-time prompt is committed as the session's newest user
    // row before the snapshot below is exported, so the seed carries it.
    let staged_prompt_refused = deliver_staged_prompt_for_live_open(service, session_id).await?;
    let (session, canonical_user_image_decoded_bytes) = service
        .export_realtime_open_session_snapshot_with_image_usage(session_id)
        .await?;
    let llm_identity = service.live_session_llm_identity(session_id).await?;
    let visible_tools = service.live_visible_tool_defs(session_id).await?;
    // 0.7.27: exact-retry + live-rewrite guards ride the open config — the
    // user-content identity lane and the transcript rewrite generation come
    // from the exported snapshot, mirroring the facade orchestrator.
    let transcript_rewrite_generation = session.transcript_rewrite_generation().map_err(|err| {
        meerkat_core::SessionError::Agent(meerkat_core::error::AgentError::InternalError(
            err.to_string(),
        ))
    })?;
    // 0.7.28: seed bounding is upstream-owned (`live/open.seed_max_chars`,
    // upstream ask 30 SHIPPED) — the windowed projection preserves the
    // enabled root context, an affordable compaction summary, and the
    // identity/tombstone/rewrite-generation/canonical-image sidecars, and
    // reports degraded continuity explicitly. This replaced meta's
    // oldest-first clamp stopgap.
    let seed_projection = match seed_max_chars {
        Some(max_chars) => {
            let window =
                meerkat::session_runtime::live_orchestration::LiveSeedWindow::new(max_chars)
                    .map_err(|err| {
                        meerkat_core::SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(err.to_string()),
                        )
                    })?;
            let projection =
                meerkat::session_runtime::live_orchestration::realtime_projection_messages_with_window(
                    &session, window,
                )
                .map_err(|err| {
                    meerkat_core::SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(err.to_string()),
                    )
                })?;
            if !matches!(
                projection.status,
                meerkat::session_runtime::live_orchestration::LiveSeedProjectionStatus::Complete
            ) {
                tracing::info!(
                    target: "meerkat_meta::live_wiring",
                    %session_id,
                    max_chars,
                    status = ?projection.status,
                    "live open seed windowed (upstream seed_max_chars)"
                );
            }
            projection.messages
        }
        None => realtime_projection_messages(&session)?,
    };
    let open_config = RealtimeSessionOpenConfig::for_open_from_messages(
        turning_mode,
        llm_identity,
        visible_tools,
        seed_projection,
        session.messages(),
    )
    .map_err(|err| {
        meerkat_core::SessionError::Agent(meerkat_core::error::AgentError::InternalError(
            err.to_string(),
        ))
    })?;
    Ok((
        open_config
            .with_open_projection_lease(open_projection_lease)
            .with_user_content_identities(session.realtime_user_content_identities())
            .with_user_content_tombstones(session.realtime_user_content_tombstones())
            .with_canonical_user_image_decoded_bytes(canonical_user_image_decoded_bytes)
            .with_transcript_rewrite_generation(transcript_rewrite_generation),
        staged_prompt_refused,
    ))
}

/// Deliver the session's staged create-time prompt for a live open, as
/// Meerkat's ordinary open does: a deferred session whose first interaction
/// is this open hands the prompt to the realtime model once, through the
/// `RunStarted` hooks a turn carrying it would run. The delivery takes the
/// session's turn boundary, which the snapshot export after it waits for
/// anyway, so it adds no wait to the open: another holder of the boundary
/// delays it and it then delivers once, and a turn in flight consumes the
/// prompt itself, leaving nothing to deliver. A session with nothing staged
/// answers at once, with no guard, actor command or store read. A hook that
/// refuses the prompt keeps it staged for the first text turn and the open
/// goes ahead without it: the answer is `true`, reported as the
/// `staged_prompt_refused` advisory.
#[cfg(not(feature = "openai-live"))]
async fn deliver_staged_prompt_for_live_open<B: SessionAgentBuilder + 'static>(
    service: &PersistentSessionService<B>,
    session_id: &SessionId,
) -> Result<bool, meerkat_core::SessionError> {
    let delivery = service
        .deliver_staged_prompt_for_live_open(session_id)
        .await?;
    if let meerkat_session::StagedPromptLiveOpenDelivery::Refused(refusal) = delivery {
        tracing::warn!(
            target: "meerkat_meta::live_wiring",
            %session_id,
            %refusal,
            "a run-start hook refused the staged create-time prompt at live open; it stays staged for the first text turn"
        );
        return Ok(true);
    }
    Ok(false)
}

/// The realtime identity that `channel_id`'s open bound on `session_id`, read
/// from its native owner, the machine's live channel binding projection. A
/// refresh never substitutes the member's text identity: an absent binding is
/// refused as an unbound channel request.
pub(crate) async fn live_refresh_channel_identity(
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
    rpc_id: Value,
) -> Result<SessionLlmIdentity, Box<JsonRpcResponse>> {
    match machine.live_channel_binding(session_id, channel_id).await {
        Ok(Some(binding)) => Ok(binding.into_identity()),
        Ok(None) => Err(Box::new(
            live_unbound_channel_request_error_response(
                rpc_id,
                machine,
                channel_id,
                meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::Refresh,
            )
            .await,
        )),
        Err(error) => Err(Box::new(live_error(
            rpc_id,
            INTERNAL_ERROR_CODE,
            format!("live refresh could not read the channel's bound identity: {error}"),
        ))),
    }
}

/// Project `session_id` for a configuration-only refresh of a channel bound to
/// `llm_identity`: the hydration-free refresh export, no open reservation and
/// no seed history, with the currentness sidecars an adapter checks to require
/// a reopen. This is the native refresh construction
/// (`RealtimeSessionOpenConfig::for_refresh_from_messages`).
async fn live_refresh_config_for_session<B: SessionAgentBuilder + 'static>(
    service: &PersistentSessionService<B>,
    session_id: &SessionId,
    llm_identity: SessionLlmIdentity,
) -> Result<RealtimeSessionOpenConfig, meerkat_core::SessionError> {
    let session = service
        .export_realtime_refresh_session_snapshot(session_id)
        .await?;
    let visible_tools = service.live_visible_tool_defs(session_id).await?;
    let transcript_rewrite_generation = session.transcript_rewrite_generation().map_err(|err| {
        meerkat_core::SessionError::Agent(meerkat_core::error::AgentError::InternalError(
            err.to_string(),
        ))
    })?;
    let refresh_config = RealtimeSessionOpenConfig::for_refresh_from_messages(
        RealtimeTurningMode::ProviderManaged,
        llm_identity,
        visible_tools,
        session.messages(),
    )
    .map_err(|err| {
        meerkat_core::SessionError::Agent(meerkat_core::error::AgentError::InternalError(
            err.to_string(),
        ))
    })?;
    Ok(refresh_config
        .with_user_content_identities(session.realtime_user_content_identities())
        .with_user_content_tombstones(session.realtime_user_content_tombstones())
        .with_transcript_rewrite_generation(transcript_rewrite_generation))
}

/// Fold the per-open `(provider, model)` selection into the projected
/// channel identity (design §6 + the downstream app's cross-provider regression).
///
/// `model` alone swaps the realtime model for this channel without touching
/// the member's text identity - the pre-existing v1 override, byte-identical
/// when `provider` is absent. A `provider` that differs from the member's
/// inherited text provider re-pairs the channel identity AND clears the
/// inherited provider-specific auth binding, so the selected provider's
/// configured default credential resolution applies (an Anthropic binding
/// must never ride into an OpenAI realtime open). A `provider` matching the
/// inherited one is a no-op beyond the model swap: the member's binding
/// stays valid for its own provider. The caller guarantees `provider` never
/// arrives without `model` (the pair is mutated together).
fn apply_live_open_identity_selection(
    llm_identity: &mut SessionLlmIdentity,
    provider: Option<Provider>,
    model: Option<String>,
) {
    if let Some(model) = model {
        llm_identity.model = model;
    }
    if let Some(provider) = provider
        && provider != llm_identity.provider
    {
        llm_identity.provider = provider;
        llm_identity.auth_binding = None;
    }
}

#[cfg(not(feature = "openai-live"))]
mod ordinary_open_helpers {
    use super::*;

    /// #176: project the factory's typed realtime audio policy into the typed
    /// [`LiveAudioConfig`] the snapshot carries. `None` when the factory does
    /// not advertise both directions of audio, so the caller fails closed
    /// rather than inventing a sample rate.
    pub(super) fn live_audio_config_from_capabilities(
        capabilities: &RealtimeCapabilities,
    ) -> Option<LiveAudioConfig> {
        let input = capabilities.audio_input_format.as_ref()?;
        let output = capabilities.audio_output_format.as_ref()?;
        Some(LiveAudioConfig {
            input_sample_rate_hz: input.sample_rate_hz,
            input_channels: u16::from(input.channels),
            output_sample_rate_hz: output.sample_rate_hz,
            output_channels: u16::from(output.channels),
        })
    }

    /// #176: derive the WS `&format=` query token from the typed audio policy.
    /// The WS transport stamps inbound binary-chunk sample rates from this
    /// token, and the only layout it accepts today is 16-bit signed LE PCM,
    /// 24 kHz, mono. Fail closed (`None`) when the resolved policy does not map
    /// — a mismatch would silently stamp the wrong rate onto every frame.
    pub(super) fn live_ws_audio_format_param(audio: &LiveAudioConfig) -> Option<&'static str> {
        const PCM_24K_MONO_RATE_HZ: u32 = 24_000;
        const PCM_24K_MONO_CHANNELS: u16 = 1;
        if audio.input_sample_rate_hz == PCM_24K_MONO_RATE_HZ
            && audio.input_channels == PCM_24K_MONO_CHANNELS
        {
            Some("pcm_24k_mono")
        } else {
            None
        }
    }

    /// A8: build a `LiveProjectionSnapshot` from the resolved open config.
    /// `snapshot_version = 0` is the open-time placeholder; the refresh path
    /// overwrites it via `host.next_snapshot_version(channel_id)` (R8).
    pub(super) fn build_live_projection_snapshot(
        session_id: &SessionId,
        open_config: &RealtimeSessionOpenConfig,
        audio_config: Option<LiveAudioConfig>,
    ) -> LiveProjectionSnapshot {
        LiveProjectionSnapshot {
            session_id: session_id.clone(),
            snapshot_version: 0,
            seed_messages: open_config.seed_messages().to_vec(),
            visible_tools: open_config.visible_tools.clone(),
            user_content_identities: open_config.user_content_identities.clone(),
            user_content_tombstones: open_config.user_content_tombstones.clone(),
            transcript_rewrite_generation: open_config.transcript_rewrite_generation,
            canonical_user_image_decoded_bytes: open_config.canonical_user_image_decoded_bytes,
            // 0.8.11: the exact canonical System payload sequence is the refresh
            // drift witness; the actual System rows ride `seed_messages` and are
            // replayed natively by provider adapters.
            canonical_system_messages: open_config.canonical_system_messages_ref().to_vec(),
            model_id: open_config.llm_identity.model.clone(),
            provider_id: open_config.llm_identity.provider,
            audio_config,
        }
    }

    /// A8: `Fresh` on empty seed history, `TranscriptOnly` once seeded.
    /// Provider-native resume is not wired by any shipped provider.
    pub(super) fn continuity_from_snapshot(
        snapshot: &LiveProjectionSnapshot,
    ) -> LiveContinuityMode {
        if snapshot.seed_messages.is_empty() {
            LiveContinuityMode::Fresh
        } else {
            LiveContinuityMode::TranscriptOnly
        }
    }

    pub(super) async fn abandon_live_open_admission(
        machine: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        channel_id: &LiveChannelId,
    ) {
        if let Err(err) = machine
            .abandon_live_open_admission(session_id, channel_id)
            .await
        {
            tracing::warn!(
                target: "meerkat_meta::live_wiring",
                ?channel_id,
                ?session_id,
                ?err,
                "generated live-open admission abandonment failed"
            );
        }
    }

    /// #355: open-failure cleanup is fail-closed, not best-effort. Attempt the
    /// generated graceful close first (retains a queryable closed-channel
    /// status); if the close authority rejects, omits the host handoff, or the
    /// host commit fails, fall through to `abandon_live_open_admission` so a
    /// failed open never leaves an orphaned machine-owned channel binding.
    pub(super) async fn close_live_channel_after_open_failure(
        host: &LiveAdapterHost,
        machine: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        channel_id: &LiveChannelId,
    ) {
        match host
            .reserve_channel_close_observation(
                channel_id,
                meerkat_core::LiveChannelCloseReason::OpenAbandoned,
            )
            .await
        {
            Ok(observation) => {
                // Reference order: the host commit requires the adapter
                // physically closed and detached first (otherwise it fails
                // closed as `CloseNotAuthorized` and this cleanup always
                // degraded to admission eviction instead of the graceful
                // close it documents). Physical-close failure keeps the
                // fail-closed eviction fallback below.
                let physically_closed =
                    match host.prepare_channel_physical_close(&observation).await {
                        Ok(()) => true,
                        Err(err) => {
                            tracing::warn!(
                                target: "meerkat_meta::live_wiring",
                                ?channel_id,
                                ?session_id,
                                ?err,
                                "physical adapter close failed during open-failure cleanup; \
                                 evicting admission"
                            );
                            false
                        }
                    };
                let committed = physically_closed
                    && commit_live_close_for_open_failure(
                        host,
                        machine,
                        session_id,
                        channel_id,
                        &observation,
                    )
                    .await;
                if !committed {
                    abandon_live_open_admission(machine, session_id, channel_id).await;
                }
            }
            Err(LiveAdapterHostError::ChannelNotFound(_)) => {
                abandon_live_open_admission(machine, session_id, channel_id).await;
            }
            Err(err) => {
                tracing::warn!(
                    target: "meerkat_meta::live_wiring",
                    ?channel_id,
                    ?session_id,
                    ?err,
                    "failed to close live channel after open failure; evicting admission"
                );
                abandon_live_open_admission(machine, session_id, channel_id).await;
            }
        }
    }

    /// Returns `true` only when the host commit succeeded (the machine-owned
    /// channel binding is cleared); `false` after logging the typed cause.
    pub(super) async fn commit_live_close_for_open_failure(
        host: &LiveAdapterHost,
        machine: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        channel_id: &LiveChannelId,
        observation: &LiveChannelCloseObservation,
    ) -> bool {
        let authority = match machine
            .resolve_live_close_result(session_id, observation)
            .await
        {
            Ok(authority) => authority,
            Err(err) => {
                tracing::warn!(
                    target: "meerkat_meta::live_wiring",
                    ?channel_id,
                    ?session_id,
                    ?err,
                    "generated live-close authority rejected open-failure cleanup; evicting admission"
                );
                return false;
            }
        };
        let Some(close_commit_authority) = authority.channel_close_commit_authority() else {
            tracing::warn!(
                target: "meerkat_meta::live_wiring",
                ?channel_id,
                ?session_id,
                "generated live-close result omitted host commit authority; evicting admission"
            );
            return false;
        };
        if let Err(err) = host
            .commit_channel_close_observation(observation, close_commit_authority)
            .await
        {
            tracing::warn!(
                target: "meerkat_meta::live_wiring",
                ?channel_id,
                ?session_id,
                ?err,
                "host live-close commit failed after generated open-failure cleanup; evicting admission"
            );
            return false;
        }
        true
    }
}

#[cfg(not(feature = "openai-live"))]
use ordinary_open_helpers::*;

/// A8: build a `LiveProjectionSnapshot` from the resolved open config.
/// `snapshot_version = 0` is the open-time placeholder; the refresh path
/// overwrites it via `host.next_snapshot_version(channel_id)` (R8).
#[cfg(feature = "openai-live")]
fn build_live_projection_snapshot(
    session_id: &SessionId,
    open_config: &RealtimeSessionOpenConfig,
) -> LiveProjectionSnapshot {
    LiveProjectionSnapshot {
        session_id: session_id.clone(),
        snapshot_version: 0,
        seed_messages: open_config.seed_messages().to_vec(),
        visible_tools: open_config.visible_tools.clone(),
        user_content_identities: open_config.user_content_identities.clone(),
        user_content_tombstones: open_config.user_content_tombstones.clone(),
        transcript_rewrite_generation: open_config.transcript_rewrite_generation,
        canonical_user_image_decoded_bytes: open_config.canonical_user_image_decoded_bytes,
        // 0.8.11: the exact canonical System payload sequence is the refresh
        // drift witness; the actual System rows ride `seed_messages` and are
        // replayed natively by provider adapters.
        canonical_system_messages: open_config.canonical_system_messages_ref().to_vec(),
        model_id: open_config.llm_identity.model.clone(),
        provider_id: open_config.llm_identity.provider,
        audio_config: None,
    }
}

#[cfg(not(feature = "openai-live"))]
#[allow(clippy::too_many_lines)]
async fn handle_live_open<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    service: &Arc<PersistentSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: GatewayLiveOpenParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    // The meta gateway mounts the WS transport only; a webrtc request is
    // a caller error, not a silent websocket fallback.
    match parsed.transport {
        None | Some(LiveOpenTransport::Websocket) => {}
        Some(_) => {
            return live_error(
                rpc_id,
                INVALID_PARAMS_CODE,
                "only the websocket live transport is supported by this gateway",
            );
        }
    }
    // Downstream app cross-provider regression: resolve the strict optional
    // `provider` selection BEFORE any session work, so an unrecognized name
    // is a pure typed parameter error (`Provider::from_name` would coerce
    // it to `Other` and fall through to a misleading downstream rejection).
    let provider_override = match parsed.provider.as_deref() {
        None => None,
        Some(name) => match Provider::parse_strict(name) {
            Some(provider) => Some(provider),
            None => {
                return live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    format!(
                        "unknown provider '{name}'; expected one of: \
                         anthropic, openai, gemini, self_hosted"
                    ),
                );
            }
        },
    };
    // The channel identity's (provider, model) pair is mutated together: a
    // provider selection without a model would pair the new provider with
    // the member's text model, which is never what a cross-provider live
    // open means.
    if provider_override.is_some() && parsed.model.is_none() {
        return live_error(
            rpc_id,
            INVALID_PARAMS_CODE,
            "live/open `provider` requires an explicit `model`",
        );
    }

    // R3-1: honor the caller's optional `turning_mode`; default
    // `ProviderManaged`. Text-only callers that drive `commit_input` must
    // pass `ExplicitCommit` (the OpenAI realtime API rejects
    // `input_audio_buffer.commit` outside explicit-commit sessions).
    let turning_mode = parsed
        .turning_mode
        .unwrap_or(RealtimeTurningMode::ProviderManaged);
    // The per-open seed_max_chars override beats the gateway-wide
    // `runtime_options.live.seed_max_chars`; windowing is upstream-owned
    // (0.7.28, ask 30 SHIPPED).
    let seed_max_chars = parsed.seed_max_chars.or(ctx.seed_max_chars);
    let (mut open_config, staged_prompt_refused) =
        match live_open_config_for_session(service, session_id, turning_mode, seed_max_chars).await
        {
            Ok(projection) => projection,
            Err(meerkat_core::SessionError::NotFound { .. }) => {
                return live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    format!("session {session_id} not found"),
                );
            }
            Err(err) => {
                return live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!("failed to build session config: {err}"),
                );
            }
        };
    if let Some(overlay) = parsed
        .instructions
        .and_then(|instructions| live_open_instruction_overlay(instructions.into_vec()))
    {
        open_config.append_ephemeral_system_overlay(overlay);
    }
    // Design §6: the member session's model decides; an explicit `model`
    // override swaps the realtime model for this channel without touching
    // the member's text identity, and an explicit `provider` re-pairs the
    // channel identity for a cross-provider open. Applied before precheck +
    // admission so both gate — and the machine binds — the identity
    // actually opened.
    apply_live_open_identity_selection(
        &mut open_config.llm_identity,
        provider_override,
        parsed.model,
    );
    // B19 precheck runs BEFORE any channel infra is minted (the reference
    // prechecks after `open_channel_with_authority` and then unwinds; with
    // the identity already resolved there is nothing to unwind here).
    if let Err(precheck_err) = precheck_identity(&open_config.llm_identity) {
        let (code, message) = match &precheck_err {
            LiveOpenPrecheckError::ModelNotRealtime { .. } => {
                (INVALID_PARAMS_CODE, precheck_err.to_string())
            }
            _ => (INTERNAL_ERROR_CODE, precheck_err.to_string()),
        };
        return live_error(rpc_id, code, message);
    }
    // B18: the factory that mints the adapter owns provider support.
    if !ctx
        .session_factory
        .supports_provider(open_config.llm_identity.provider)
    {
        return live_error(
            rpc_id,
            INTERNAL_ERROR_CODE,
            format!(
                "provider {} has no live adapter wired in this build",
                open_config.llm_identity.provider.as_str()
            ),
        );
    }

    // One machine-owned lifecycle boundary spans generated admission and all
    // provider/transport materialization. Retaining the lease prevents a
    // concurrent retire from proving the live registry empty before this open
    // installs its channel.
    let _live_lifecycle_lease = match machine.acquire_live_open_lifecycle_lease(session_id).await {
        Ok(lease) => lease,
        Err(err) => {
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("live open lifecycle authority unavailable: {err}"),
            );
        }
    };

    let live_open_identity = open_config.llm_identity.clone();
    let candidate_channel_id = LiveChannelId::random_uuid();
    let open_authority = match machine
        .resolve_live_open_admission(session_id, &candidate_channel_id, &live_open_identity)
        .await
    {
        Ok(authority) => authority,
        Err(err) => {
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("live open authority rejected admission: {err}"),
            );
        }
    };
    if !open_authority.admitted() {
        use meerkat_runtime::meerkat_machine::dsl::LiveOpenAdmissionRejection;
        return match open_authority.rejection() {
            Some(LiveOpenAdmissionRejection::AlreadyBound) => live_error(
                rpc_id,
                INVALID_PARAMS_CODE,
                format!("session {session_id} already has an active live channel"),
            ),
            Some(LiveOpenAdmissionRejection::ChannelAlreadyBound) => live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("generated duplicate live channel id {candidate_channel_id}"),
            ),
            Some(LiveOpenAdmissionRejection::RevokedChannelId) => live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!(
                    "generated live channel id {candidate_channel_id} was previously revoked, \
                     so admission is refused rather than reusing a revoked identity"
                ),
            ),
            Some(LiveOpenAdmissionRejection::LifecycleClosed) => live_error(
                rpc_id,
                INVALID_PARAMS_CODE,
                "session lifecycle is closed to live channel admission",
            ),
            None => live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                "live open authority rejected admission without a reason",
            ),
        };
    }

    let Some(channel_open_authority) = open_authority.channel_open_authority() else {
        abandon_live_open_admission(machine, session_id, &candidate_channel_id).await;
        return live_error(
            rpc_id,
            INTERNAL_ERROR_CODE,
            "live open admission was accepted without a generated host handoff",
        );
    };

    let channel_id = match ctx
        .host
        .open_channel_with_authority(channel_open_authority)
        .await
    {
        Ok(ch) => ch,
        Err(LiveAdapterHostError::SessionAlreadyBound(sid)) => {
            abandon_live_open_admission(machine, session_id, &candidate_channel_id).await;
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!(
                    "live host transport cache still has active channel for session {sid} \
                     after generated admission"
                ),
            );
        }
        Err(err) => {
            abandon_live_open_admission(machine, session_id, &candidate_channel_id).await;
            return live_error(rpc_id, INTERNAL_ERROR_CODE, err.to_string());
        }
    };

    // #176: resolve the typed audio policy from the factory's capabilities
    // (the provider/model audio-format owner) — the WS `&format=` token and
    // the snapshot `audio_config` both read this single typed value.
    let resolved_audio_config =
        live_audio_config_from_capabilities(&ctx.session_factory.capabilities());
    // E25: open the provider-native adapter directly. The factory already
    // consumed the replay seed (including any per-open System overlay), so no
    // `LiveAdapterCommand::Open` is dispatched afterwards (R2: re-seeding
    // would compound the provider transcript).
    let capabilities: LiveChannelCapabilities;
    let continuity: LiveContinuityMode = match ctx
        .session_factory
        .open_live_adapter(&open_config)
        .await
    {
        Ok(adapter) => {
            // P2#3: the adapter's real capability set, queried before the
            // host takes canonical ownership of the Arc.
            capabilities = adapter.capabilities();
            if let Err(err) = ctx.host.attach_adapter(&channel_id, adapter).await {
                close_live_channel_after_open_failure(&ctx.host, machine, session_id, &channel_id)
                    .await;
                return live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!("failed to attach adapter: {err}"),
                );
            }
            let snapshot = build_live_projection_snapshot(
                session_id,
                &open_config,
                resolved_audio_config.clone(),
            );
            continuity_from_snapshot(&snapshot)
        }
        Err(err) => {
            close_live_channel_after_open_failure(&ctx.host, machine, session_id, &channel_id)
                .await;
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("failed to open provider session: {err}"),
            );
        }
    };

    let token = match ctx
        .ws_state
        .mint_token(session_id, channel_id.clone())
        .await
    {
        Ok(token) => token,
        Err(err) => {
            close_live_channel_after_open_failure(&ctx.host, machine, session_id, &channel_id)
                .await;
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("live WebSocket token authority rejected issue: {err}"),
            );
        }
    };
    let token_str = token.to_string();
    let Some(audio_config) = resolved_audio_config.as_ref() else {
        close_live_channel_after_open_failure(&ctx.host, machine, session_id, &channel_id).await;
        return live_error(
            rpc_id,
            INTERNAL_ERROR_CODE,
            "live websocket transport requires a resolved audio policy; no realtime \
             factory audio format was available",
        );
    };
    let Some(format_param) = live_ws_audio_format_param(audio_config) else {
        close_live_channel_after_open_failure(&ctx.host, machine, session_id, &channel_id).await;
        return live_error(
            rpc_id,
            INTERNAL_ERROR_CODE,
            format!(
                "resolved live audio policy (input {}Hz/{}ch) has no websocket binary \
                 format the transport can negotiate",
                audio_config.input_sample_rate_hz, audio_config.input_channels,
            ),
        );
    };
    // G38: the bearer token is pinned to the channel via the `channel`
    // query param so a leaked token cannot replay against another channel.
    let transport = LiveTransportBootstrap::Websocket {
        url: format!(
            "{base_url}{path}?token={token_str}&channel={channel_id}&format={format_param}",
            base_url = ctx.ws_base_url,
            path = meerkat_live::LIVE_WS_PATH,
        ),
        token: token_str,
    };

    // CC5/CC6/G8: project core typed shapes into the wire mirrors at the
    // boundary so SDK codegen sees typed unions, byte-compatible payloads.
    let result = LiveOpenResult {
        channel_id: channel_id.to_string(),
        transport: transport.into(),
        capabilities: capabilities.into(),
        continuity: continuity.into(),
        // The open went ahead without a staged create-time prompt that a
        // run-start hook refused: an advisory, not a failure.
        advisories: if staged_prompt_refused {
            vec![meerkat_contracts::WireLiveOpenAdvisory::StagedPromptRefused]
        } else {
            Vec::new()
        },
    };
    match serde_json::to_value(result) {
        Ok(value) => live_success(rpc_id, value),
        Err(err) => live_error(
            rpc_id,
            INTERNAL_ERROR_CODE,
            format!("failed to serialize LiveOpenResult: {err}"),
        ),
    }
}

/// Take the gateway's single live voice path for this member before opening,
/// closing whichever other door held it ("latest engaged wins"), then bind the
/// opened channel to the engagement. A loser that was preempted while its own
/// open was still in flight closes what it opened and reports the reason.
#[cfg(feature = "openai-live")]
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors handle_live_open's explicit authority inputs plus the machine the closer probes"
)]
async fn handle_live_open_arbitrated<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    ctx_machine: &Arc<MeerkatMachine>,
    shared_live_host: &Arc<ServiceMemberLiveHost<B>>,
    capability_provider: &LiveCapabilityProvider,
    session_id: &SessionId,
    canonical_target_identity: Option<String>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let identity = canonical_target_identity
        .clone()
        .or_else(|| {
            params
                .get("identity")
                .or_else(|| params.get("member_id"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| session_id.to_string());
    let closer: LiveOwnerCloser = {
        let host = Arc::clone(shared_live_host);
        let machine = Arc::clone(ctx_machine);
        let authority = capability_provider.open_authority_arc();
        Arc::new(move |_reason, channel| {
            let host = Arc::clone(&host);
            let machine = Arc::clone(&machine);
            let authority = authority.clone();
            Box::pin(async move {
                let Some(channel) = channel else {
                    return Ok(());
                };
                let channel = LiveChannelId::new(&channel);
                // A dropped WebSocket closes the channel inside meerkat-live
                // without passing through this door; nothing is left to close.
                if machine
                    .live_session_for_active_channel(&channel)
                    .await
                    .is_none()
                {
                    return Ok(());
                }
                host.close_live_channel(authority.as_deref(), &channel)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
        })
    };
    let (lease, preempted) = match ctx
        .arbiter
        .engage(
            LiveOwner::ExternalLive {
                identity,
                channel_id: None,
            },
            closer,
        )
        .await
    {
        Ok(engaged) => engaged,
        Err(error) => {
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("live open could not take the voice path: {error}"),
            );
        }
    };
    let mut response = handle_live_open(
        ctx,
        shared_live_host,
        capability_provider,
        session_id,
        canonical_target_identity,
        params,
        rpc_id.clone(),
    )
    .await;
    let opened_channel = response
        .result
        .as_ref()
        .filter(|_| response.error.is_none())
        .and_then(|value| value.get("channel_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let Some(channel_id) = opened_channel else {
        ctx.arbiter.release(lease);
        return response;
    };
    ctx.arbiter.bind_channel(lease, &channel_id);
    if let Some(reason) = ctx.arbiter.take_superseded(lease) {
        // Another door won the race while this open was in flight. Close the
        // channel we just opened through our own sequence and say why.
        let channel = LiveChannelId::new(&channel_id);
        if let Err(error) = shared_live_host
            .close_live_channel(capability_provider.open_authority(), &channel)
            .await
        {
            tracing::warn!(
                channel_id = %channel_id,
                error = %error,
                "superseded live open could not close its own channel"
            );
        }
        ctx.arbiter.record_close_reason(&channel_id, reason);
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: rpc_id,
            result: None,
            error: Some(
                JsonRpcError::new(
                    INTERNAL_ERROR_CODE,
                    "live open was superseded before it activated",
                )
                .with_data(
                    serde_json::json!({ "kind": "live_superseded", "reason": reason.as_str() }),
                ),
            ),
        };
    }
    if let (Some(previous), Some(result)) = (preempted, response.result.as_mut()) {
        result["superseded"] = previous.to_wire();
    }
    response
}

/// After a `meta/live/close`, free the voice path if the closed channel held
/// it, and report the typed reason when the arbiter closed that channel.
#[cfg(feature = "openai-live")]
fn release_live_owner_after_close(
    ctx: &GatewayLiveContext,
    params: &Value,
    mut response: JsonRpcResponse,
) -> JsonRpcResponse {
    let Some(channel_id) = params.get("channel_id").and_then(Value::as_str) else {
        return response;
    };
    if response.error.is_none() {
        ctx.arbiter.release_channel(channel_id);
    }
    if let (Some(reason), Some(result)) = (
        ctx.arbiter.close_reason(channel_id),
        response.result.as_mut(),
    ) {
        result["close_reason"] = Value::String(reason.as_str().to_string());
    }
    response
}

#[cfg(feature = "openai-live")]
#[allow(clippy::too_many_lines)]
async fn handle_live_open<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    shared_live_host: &Arc<ServiceMemberLiveHost<B>>,
    capability_provider: &LiveCapabilityProvider,
    session_id: &SessionId,
    canonical_target_identity: Option<String>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    #[cfg(not(feature = "openai-live"))]
    let _ = &canonical_target_identity;
    // This is a projection label only, never an ownership witness. Durable
    // target resolution and channel ownership remain upstream/machine-owned.
    let legacy_target_identity = params
        .get("identity")
        .or_else(|| params.get("member_id"))
        .or_else(|| params.get("session_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map_or_else(|| session_id.to_string(), ToString::to_string);
    let parsed: GatewayLiveOpenParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    #[cfg(not(feature = "openai-live"))]
    let _ = capability_provider;
    #[cfg(feature = "openai-live")]
    if let Some(execution_identity) = parsed.execution_identity.as_ref() {
        let Some(target_identity) = canonical_target_identity else {
            return live_error(
                rpc_id,
                INVALID_PARAMS_CODE,
                "strict live/open requires canonical durable identity authority",
            );
        };
        if parsed.model.is_some() || parsed.provider.is_some() {
            return live_error(
                rpc_id,
                INVALID_PARAMS_CODE,
                "execution_identity conflicts with legacy top-level model/provider",
            );
        }
        if parsed.instructions.is_some() {
            return live_error(
                rpc_id,
                INVALID_PARAMS_CODE,
                "strict experimental live/open does not accept raw instructions; select a registered execution_identity.profile_id",
            );
        }
        let Some(open_authority) = capability_provider.open_authority_arc() else {
            return live_error(
                rpc_id,
                crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
                format!(
                    "capability {} is not available",
                    crate::live_contracts::LIVE_EXECUTION_IDENTITY_V1
                ),
            );
        };
        let seed_window = match parsed.seed_max_chars.or(ctx.seed_max_chars) {
            Some(max_chars) => match LiveSeedWindow::new(max_chars) {
                Ok(window) => Some(window),
                Err(error) => {
                    return live_error(
                        rpc_id,
                        INVALID_PARAMS_CODE,
                        format!("invalid seed_max_chars: {error}"),
                    );
                }
            },
            None => None,
        };
        let open_started = std::time::Instant::now();
        let result = match shared_live_host
            .open_with_execution_identity(
                open_authority.as_ref(),
                session_id,
                execution_identity,
                parsed.turning_mode,
                seed_window,
                parsed.transport,
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                tracing::info!(
                    target: "meerkat_meta::console_voice::timing",
                    %session_id,
                    elapsed_ms = u64::try_from(open_started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    error = ?error,
                    "live open authority failed"
                );
                return experimental_live_open_error_response(rpc_id, error);
            }
        };
        let channel_id = result.channel_id().clone();
        tracing::info!(
            target: "meerkat_meta::console_voice::timing",
            %session_id,
            %channel_id,
            elapsed_ms = u64::try_from(open_started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "live open authority completed"
        );
        let execution_mode = local_live_execution_mode(result.execution_mode());
        let pending_receipt = result.pending_receipt().to_string();
        let open = result.into_open();
        return match serde_json::to_value(crate::live_contracts::PendingLiveChannelHandle::new(
            target_identity,
            execution_mode,
            pending_receipt,
            open,
        )) {
            Ok(value) => {
                let cleanup: Arc<dyn LiveOpenPublicationCleanup> =
                    Arc::new(LiveOpenPublicationCleanupOwner {
                        host: Arc::clone(shared_live_host),
                        authority: Arc::clone(&open_authority),
                        session: session_id.clone(),
                        channel: channel_id.clone(),
                    });
                let custody = LiveOpenResponseDeliveryCustody::new(cleanup);
                if let Err(error) = retain_live_open_response_delivery(custody).await {
                    live_error(rpc_id, INTERNAL_ERROR_CODE, error)
                } else {
                    live_success(rpc_id, value)
                }
            }
            Err(error) => {
                let cleanup = shared_live_host
                    .cleanup_execution_identity_publication_failure(
                        open_authority.as_ref(),
                        session_id,
                        &channel_id,
                    )
                    .await;
                match cleanup {
                    Ok(()) => live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!("failed to serialize LiveChannelHandle: {error}"),
                    ),
                    Err(cleanup_error) => live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!(
                            "failed to serialize LiveChannelHandle and cleanup failed: {cleanup_error}"
                        ),
                    ),
                }
            }
        };
    }
    // The meta gateway mounts the WS transport only; a webrtc request is
    // a caller error, not a silent websocket fallback.
    match parsed.transport {
        None | Some(LiveOpenTransport::Websocket) => {}
        Some(_) => {
            return live_error(
                rpc_id,
                INVALID_PARAMS_CODE,
                "only the websocket live transport is supported by this gateway",
            );
        }
    }
    // Downstream app cross-provider regression: resolve the strict optional
    // `provider` selection BEFORE any session work, so an unrecognized name
    // is a pure typed parameter error (`Provider::from_name` would coerce
    // it to `Other` and fall through to a misleading downstream rejection).
    let provider_override = match parsed.provider.as_deref() {
        None => None,
        Some(name) => match Provider::parse_strict(name) {
            Some(provider) => Some(provider),
            None => {
                return live_error(
                    rpc_id,
                    INVALID_PARAMS_CODE,
                    format!(
                        "unknown provider '{name}'; expected one of: \
                         anthropic, openai, gemini, self_hosted"
                    ),
                );
            }
        },
    };
    // The channel identity's (provider, model) pair is mutated together: a
    // provider selection without a model would pair the new provider with
    // the member's text model, which is never what a cross-provider live
    // open means.
    if provider_override.is_some() && parsed.model.is_none() {
        return live_error(
            rpc_id,
            INVALID_PARAMS_CODE,
            "live/open `provider` requires an explicit `model`",
        );
    }

    // R3-1: honor the caller's optional `turning_mode`; default
    // `ProviderManaged`. Text-only callers that drive `commit_input` must
    // pass `ExplicitCommit` (the OpenAI realtime API rejects
    // `input_audio_buffer.commit` outside explicit-commit sessions).
    let turning_mode = parsed
        .turning_mode
        .unwrap_or(RealtimeTurningMode::ProviderManaged);
    // The legacy provider/model/overlay shape is applied only to the typed,
    // session-sealed projection. Shared Meerkat retains ownership of every
    // effectful S5-S12 step and validates the projection seal before the
    // lifecycle lease, machine admission, provider factory, or token mint.
    let seed_window = match parsed.seed_max_chars.or(ctx.seed_max_chars) {
        Some(max_chars) => match LiveSeedWindow::new(max_chars) {
            Ok(window) => Some(window),
            Err(error) => {
                return live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!("failed to build session config: {error}"),
                );
            }
        },
        None => None,
    };
    let mut projection = match shared_live_host
        .prepare_open_projection(session_id, turning_mode, seed_window)
        .await
    {
        Ok(projection) => projection,
        Err(error) => {
            return live_open_error_response(rpc_id, LiveOpenError::OpenConfig(error));
        }
    };
    if let Some(overlay) = parsed
        .instructions
        .and_then(|instructions| live_open_instruction_overlay(instructions.into_vec()))
    {
        projection.append_ephemeral_system_overlay(overlay);
    }
    // Design §6: the member session's model decides; an explicit `model`
    // override swaps the realtime model for this channel without touching
    // the member's text identity, and an explicit `provider` re-pairs the
    // channel identity for a cross-provider open. Applied before precheck +
    // admission so both gate — and the machine binds — the identity
    // actually opened.
    apply_live_open_identity_selection(
        &mut projection.open_config.llm_identity,
        provider_override,
        parsed.model,
    );
    match shared_live_host
        .open_from_projection(session_id, projection, parsed.transport)
        .await
    {
        Ok(result) => {
            match serde_json::to_value(crate::live_contracts::LiveChannelHandle::from_open_result(
                legacy_target_identity,
                result,
            )) {
                Ok(value) => live_success(rpc_id, value),
                Err(err) => live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!("failed to serialize LiveOpenResult: {err}"),
                ),
            }
        }
        Err(error) => live_open_error_response(rpc_id, error),
    }
}

#[cfg(feature = "openai-live")]
fn experimental_live_open_error_response(
    rpc_id: Value,
    error: meerkat::session_runtime::live_orchestration::ExperimentalLiveChannelOpenError,
) -> JsonRpcResponse {
    use meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError;
    use meerkat::session_runtime::live_orchestration::ExperimentalLiveChannelOpenError;

    let detail = error.to_string();
    match error {
        ExperimentalLiveChannelOpenError::InvalidTransport
        | ExperimentalLiveChannelOpenError::Authority(
            ExperimentalLiveOpenAuthorityError::InvalidExecutionIdentity,
        ) => live_error(rpc_id, INVALID_PARAMS_CODE, detail),
        ExperimentalLiveChannelOpenError::Authority(
            ExperimentalLiveOpenAuthorityError::Unavailable
            | ExperimentalLiveOpenAuthorityError::AccessDenied
            | ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable
            | ExperimentalLiveOpenAuthorityError::MemberIneligible
            | ExperimentalLiveOpenAuthorityError::BindingUseDenied
            | ExperimentalLiveOpenAuthorityError::AdmissionFailed,
        ) => live_error(rpc_id, crate::rpc::CAPABILITY_UNAVAILABLE_CODE, detail),
        ExperimentalLiveChannelOpenError::ExecutionProfile(_) => {
            live_error(rpc_id, crate::rpc::CAPABILITY_UNAVAILABLE_CODE, detail)
        }
        ExperimentalLiveChannelOpenError::Summary(_) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: rpc_id,
            result: None,
            error: Some(
                JsonRpcError::new(INTERNAL_ERROR_CODE, detail)
                    .with_data(serde_json::json!({ "kind": "live_summary_rejected" })),
            ),
        },
        ExperimentalLiveChannelOpenError::Open(open_error) => {
            live_open_error_response(rpc_id, open_error)
        }
        ExperimentalLiveChannelOpenError::Projection(projection_error) => {
            live_open_error_response(rpc_id, LiveOpenError::OpenConfig(projection_error))
        }
        ExperimentalLiveChannelOpenError::Authority(
            ExperimentalLiveOpenAuthorityError::ChannelBindingFailed,
        )
        | ExperimentalLiveChannelOpenError::BindingCleanup { .. } => {
            live_error(rpc_id, INTERNAL_ERROR_CODE, detail)
        }
    }
}

fn live_refresh_result_from_machine_authority(
    authority: &meerkat_runtime::meerkat_machine::LiveRefreshResultAuthority,
) -> LiveRefreshResult {
    // Exhaustive: a future generated status variant forces a compile error
    // here, so the wire projection can never silently misreport.
    match authority.status {
        meerkat_runtime::meerkat_machine::dsl::LiveRefreshPublicStatus::Queued => {
            LiveRefreshResult::queued()
        }
    }
}

#[cfg(not(feature = "openai-live"))]
fn live_close_result_from_machine_authority(
    authority: &meerkat_runtime::meerkat_machine::LiveCloseResultAuthority,
) -> LiveCloseResult {
    match authority.status {
        meerkat_runtime::meerkat_machine::dsl::LiveClosePublicStatus::Closed => {
            LiveCloseResult::closed()
        }
    }
}

/// #234: typed result shapes for command acceptances, projected only from
/// generated machine authority. Serde failures surface as `String` so the
/// caller maps them onto the RPC error channel.
fn live_command_result_from_machine_authority(
    authority: &meerkat_runtime::meerkat_machine::LiveCommandResultAuthority,
    expected: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
) -> Result<Value, String> {
    use meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind;

    if authority.command != expected {
        return Err(format!(
            "LiveCommandResultResolved emitted command {:?} for expected {:?}",
            authority.command, expected
        ));
    }

    match expected {
        LiveCommandPublicKind::SendInput => serde_json::to_value(LiveSendInputResult::sent())
            .map_err(|err| format!("failed to serialize LiveSendInputResult: {err}")),
        LiveCommandPublicKind::CommitInput => {
            serde_json::to_value(LiveCommitInputResult::committed())
                .map_err(|err| format!("failed to serialize LiveCommitInputResult: {err}"))
        }
        LiveCommandPublicKind::Interrupt => {
            serde_json::to_value(LiveInterruptResult::interrupted())
                .map_err(|err| format!("failed to serialize LiveInterruptResult: {err}"))
        }
        LiveCommandPublicKind::TruncateAssistantOutput => {
            serde_json::to_value(LiveTruncateResult::truncated())
                .map_err(|err| format!("failed to serialize LiveTruncateResult: {err}"))
        }
        // Unconditional: Meerkat 0.8.30 carries this variant regardless of the
        // experimental feature, so gating the arm left default builds with a
        // non-exhaustive match. The refusal holds in both builds.
        LiveCommandPublicKind::CompleteAssistantPlayback => Err(
            "assistant playback terminal must use Meerkat's sealed output-handle facade"
                .to_string(),
        ),
    }
}

fn live_status_result_from_machine_authority(
    channel_id: String,
    authority: &meerkat_runtime::meerkat_machine::LiveChannelStatusAuthority,
) -> Result<LiveStatusResult, String> {
    Ok(LiveStatusResult {
        channel_id,
        status: wire_live_status_from_machine_authority(authority)?,
        // The machine authority carries no provider input-latency
        // measurement (meerkat 0.8.50 added the field, #1400).
        provider_input_latency: None,
    })
}

fn wire_live_status_from_machine_authority(
    authority: &meerkat_runtime::meerkat_machine::LiveChannelStatusAuthority,
) -> Result<WireLiveAdapterStatus, String> {
    use meerkat_runtime::meerkat_machine::dsl::LiveChannelPublicStatus;

    match authority.status {
        LiveChannelPublicStatus::Idle => Ok(WireLiveAdapterStatus::Idle),
        LiveChannelPublicStatus::Opening => Ok(WireLiveAdapterStatus::Opening),
        LiveChannelPublicStatus::Ready => Ok(WireLiveAdapterStatus::Ready),
        LiveChannelPublicStatus::Closing => Ok(WireLiveAdapterStatus::Closing),
        LiveChannelPublicStatus::Closed => Ok(WireLiveAdapterStatus::closed()),
        LiveChannelPublicStatus::Degraded => {
            let reason = authority.degradation_reason.ok_or_else(|| {
                "LiveChannelStatusResolved emitted degraded status without reason".to_string()
            })?;
            Ok(WireLiveAdapterStatus::Degraded {
                reason: wire_live_degradation_reason_from_machine_authority(
                    reason,
                    authority.degradation_detail.as_deref(),
                ),
            })
        }
    }
}

fn wire_live_degradation_reason_from_machine_authority(
    reason: meerkat_runtime::meerkat_machine::dsl::LiveChannelDegradationReason,
    detail: Option<&str>,
) -> WireLiveDegradationReason {
    use meerkat_runtime::meerkat_machine::dsl::LiveChannelDegradationReason;

    match reason {
        LiveChannelDegradationReason::RateLimited => WireLiveDegradationReason::RateLimited,
        LiveChannelDegradationReason::ProviderThrottled => {
            WireLiveDegradationReason::ProviderThrottled
        }
        LiveChannelDegradationReason::NetworkUnstable => WireLiveDegradationReason::NetworkUnstable,
        LiveChannelDegradationReason::Other => WireLiveDegradationReason::Other {
            detail: detail.unwrap_or_default().to_string(),
        },
        LiveChannelDegradationReason::Unknown => WireLiveDegradationReason::Unknown {
            debug: detail
                .unwrap_or("unknown live channel degradation")
                .to_string(),
        },
    }
}

fn live_command_rejection_response_from_machine_authority(
    rpc_id: Value,
    authority: &meerkat_runtime::meerkat_machine::LiveCommandRejectionAuthority,
    expected: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
    channel_id: &LiveChannelId,
    host_error: &LiveAdapterHostError,
) -> JsonRpcResponse {
    use meerkat_runtime::meerkat_machine::dsl::{
        LiveCommandRejectionPublicErrorClass, LiveCommandRejectionReason,
    };

    if authority.command != expected {
        return live_error(
            rpc_id,
            INTERNAL_ERROR_CODE,
            format!(
                "LiveCommandRejectionResolved emitted command {:?} for expected {:?}",
                authority.command, expected
            ),
        );
    }

    let code = match authority.public_error_class {
        LiveCommandRejectionPublicErrorClass::InvalidParams => INVALID_PARAMS_CODE,
        LiveCommandRejectionPublicErrorClass::InternalError => INTERNAL_ERROR_CODE,
    };
    let message = match authority.rejection {
        LiveCommandRejectionReason::ChannelNotFound => format!("channel {channel_id} not found"),
        LiveCommandRejectionReason::NoAdapter => {
            format!("channel {channel_id} has no adapter attached")
        }
        LiveCommandRejectionReason::ChannelNotReady
        | LiveCommandRejectionReason::UnsupportedCommand
        | LiveCommandRejectionReason::AdapterError
        | LiveCommandRejectionReason::InternalHostError => host_error.to_string(),
    };
    live_error(rpc_id, code, message)
}

async fn live_command_error_response(
    rpc_id: Value,
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
    command: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
    host_error: &LiveAdapterHostError,
) -> JsonRpcResponse {
    let authority = match machine
        .resolve_live_command_rejection_result(session_id, channel_id, command, host_error)
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("live command rejection authority rejected result: {error}"),
            );
        }
    };
    live_command_rejection_response_from_machine_authority(
        rpc_id, &authority, command, channel_id, host_error,
    )
}

async fn live_unbound_command_error_response(
    rpc_id: Value,
    machine: &Arc<MeerkatMachine>,
    channel_id: &LiveChannelId,
    command: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
) -> JsonRpcResponse {
    let authority = match machine
        .resolve_unbound_live_command_rejection_result(channel_id, command)
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("unbound live command rejection authority rejected result: {error}"),
            );
        }
    };
    let host_error = LiveAdapterHostError::ChannelNotFound(channel_id.clone());
    live_command_rejection_response_from_machine_authority(
        rpc_id,
        &authority,
        command,
        channel_id,
        &host_error,
    )
}

fn live_channel_request_rejection_response_from_machine_authority(
    rpc_id: Value,
    authority: &meerkat_runtime::meerkat_machine::LiveChannelRequestRejectionAuthority,
    expected: meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind,
    channel_id: &LiveChannelId,
    detail: Option<String>,
) -> JsonRpcResponse {
    use meerkat_runtime::meerkat_machine::dsl::{
        LiveChannelRequestRejectionPublicErrorClass, LiveChannelRequestRejectionReason,
    };

    if authority.request != expected {
        return live_error(
            rpc_id,
            INTERNAL_ERROR_CODE,
            format!(
                "LiveChannelRequestRejectionResolved emitted request {:?} for expected {:?}",
                authority.request, expected
            ),
        );
    }

    let code = match authority.public_error_class {
        LiveChannelRequestRejectionPublicErrorClass::InvalidParams => INVALID_PARAMS_CODE,
        LiveChannelRequestRejectionPublicErrorClass::InternalError => INTERNAL_ERROR_CODE,
    };
    let message = match authority.rejection {
        LiveChannelRequestRejectionReason::ChannelNotFound => {
            format!("channel {channel_id} not found")
        }
        LiveChannelRequestRejectionReason::NoAdapter => {
            format!("channel {channel_id} has no adapter attached")
        }
        LiveChannelRequestRejectionReason::InvalidToken
        | LiveChannelRequestRejectionReason::InvalidPayload
        | LiveChannelRequestRejectionReason::WebrtcAnswerError
        | LiveChannelRequestRejectionReason::InternalHostError => detail.unwrap_or_else(|| {
            format!(
                "live channel request {:?} rejected for channel {}",
                authority.request, channel_id
            )
        }),
    };
    live_error(rpc_id, code, message)
}

async fn live_channel_request_error_response(
    rpc_id: Value,
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    channel_id: &LiveChannelId,
    request: meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind,
    host_error: &LiveAdapterHostError,
) -> JsonRpcResponse {
    let authority = match machine
        .resolve_live_channel_request_rejection_result(session_id, channel_id, request, host_error)
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("live channel request rejection authority rejected result: {error}"),
            );
        }
    };
    live_channel_request_rejection_response_from_machine_authority(
        rpc_id,
        &authority,
        request,
        channel_id,
        Some(host_error.to_string()),
    )
}

async fn live_unbound_channel_request_error_response(
    rpc_id: Value,
    machine: &Arc<MeerkatMachine>,
    channel_id: &LiveChannelId,
    request: meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind,
) -> JsonRpcResponse {
    let authority = match machine
        .resolve_unbound_live_channel_request_rejection_result(channel_id, request)
        .await
    {
        Ok(authority) => authority,
        Err(error) => {
            return live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!(
                    "unbound live channel request rejection authority rejected result: {error}"
                ),
            );
        }
    };
    let host_error = LiveAdapterHostError::ChannelNotFound(channel_id.clone());
    live_channel_request_rejection_response_from_machine_authority(
        rpc_id,
        &authority,
        request,
        channel_id,
        Some(host_error.to_string()),
    )
}

async fn handle_live_status(
    ctx: &GatewayLiveContext,
    machine: &Arc<MeerkatMachine>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: LiveChannelParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);

    let request_kind = meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::Status;
    // A channel the voice-path arbiter closed may already be unbound from
    // its session. Its owner still deserves the typed reason, so report the
    // closed status with it instead of an unbound-channel error.
    let superseded = ctx.arbiter.close_reason(&parsed.channel_id);
    let Some(session_id) = machine.live_session_for_status_channel(&channel_id).await else {
        if let Some(reason) = superseded {
            return live_success(
                rpc_id,
                serde_json::json!({
                    "channel_id": parsed.channel_id,
                    "status": WireLiveAdapterStatus::closed(),
                    "close_reason": reason.as_str(),
                }),
            );
        }
        return live_unbound_channel_request_error_response(
            rpc_id,
            machine,
            &channel_id,
            request_kind,
        )
        .await;
    };

    match ctx.host.channel_status_observation(&channel_id).await {
        Ok(observation) => {
            let authority = match machine
                .resolve_live_channel_status_result(&session_id, &observation)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!("live status authority rejected result: {error}"),
                    );
                }
            };
            let result =
                match live_status_result_from_machine_authority(parsed.channel_id, &authority) {
                    Ok(result) => result,
                    Err(error) => return live_error(rpc_id, INTERNAL_ERROR_CODE, error),
                };
            match serde_json::to_value(result) {
                Ok(mut value) => {
                    if let Some(reason) = superseded {
                        value["close_reason"] = Value::String(reason.as_str().to_string());
                    }
                    live_success(rpc_id, value)
                }
                Err(err) => live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!("failed to serialize LiveStatusResult: {err}"),
                ),
            }
        }
        Err(err) => {
            live_channel_request_error_response(
                rpc_id,
                machine,
                &session_id,
                &channel_id,
                request_kind,
                &err,
            )
            .await
        }
    }
}

#[cfg(not(feature = "openai-live"))]
async fn handle_live_close(
    ctx: &GatewayLiveContext,
    machine: &Arc<MeerkatMachine>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: LiveChannelParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);

    let request_kind = meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::Close;
    let Some(session_id) = machine.live_session_for_active_channel(&channel_id).await else {
        return live_unbound_channel_request_error_response(
            rpc_id,
            machine,
            &channel_id,
            request_kind,
        )
        .await;
    };

    match ctx
        .host
        .reserve_channel_close_observation(
            &channel_id,
            meerkat_core::LiveChannelCloseReason::ClientRequested,
        )
        .await
    {
        Ok(observation) => {
            // Reference order (`close_live_channel` in the facade's live
            // orchestration): reserve -> PHYSICAL close -> generated close
            // authority -> host commit. The host commit fails closed
            // (`CloseNotAuthorized`) unless the adapter was physically
            // closed and detached first; the original port omitted this
            // step, so every RPC-initiated close of an attached channel
            // died typed instead of closing.
            if let Err(error) = ctx.host.prepare_channel_physical_close(&observation).await {
                return live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!(
                        "physical adapter close failed before generated terminal authority: {error}"
                    ),
                );
            }
            let authority = match machine
                .resolve_live_close_result(&session_id, &observation)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!("live close authority rejected result: {error}"),
                    );
                }
            };
            let Some(close_commit_authority) = authority.channel_close_commit_authority() else {
                return live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    "live close authority omitted host commit handoff",
                );
            };
            if let Err(error) = ctx
                .host
                .commit_channel_close_observation(&observation, close_commit_authority)
                .await
            {
                return live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!("live close host commit failed after generated authority: {error}"),
                );
            }
            let result = live_close_result_from_machine_authority(&authority);
            match serde_json::to_value(result) {
                Ok(body) => live_success(rpc_id, body),
                Err(error) => live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!("live close authority projection failed: {error}"),
                ),
            }
        }
        Err(err) => {
            live_channel_request_error_response(
                rpc_id,
                machine,
                &session_id,
                &channel_id,
                request_kind,
                &err,
            )
            .await
        }
    }
}

/// Whether a channel that the live host no longer treats as active has a
/// recorded close status, so a repeated `meta/live/close` is idempotent.
#[cfg(feature = "openai-live")]
async fn live_channel_close_already_complete(
    machine: &MeerkatMachine,
    channel_id: &LiveChannelId,
) -> bool {
    machine
        .live_session_for_active_channel(channel_id)
        .await
        .is_none()
        && machine
            .live_session_for_status_channel(channel_id)
            .await
            .is_some()
}

/// JSON-RPC code for a failed `meta/live/close`. A binding or custody
/// mismatch is the caller's input; a physical transport, terminal projection,
/// or semantic verb failure is the gateway's, and a caller may retry it.
#[cfg(feature = "openai-live")]
fn live_close_error_code(error: &meerkat::surface::ExperimentalLiveChannelCloseError) -> i64 {
    use meerkat::surface::ExperimentalLiveChannelCloseError as CloseError;
    match error {
        CloseError::BindingMismatch | CloseError::LifecycleAuthority(_) => INVALID_PARAMS_CODE,
        _ => INTERNAL_ERROR_CODE,
    }
}

#[cfg(feature = "openai-live")]
async fn handle_live_close(
    ctx: &GatewayLiveContext,
    machine: &Arc<MeerkatMachine>,
    shared_live_host: &ServiceMemberLiveHost<impl SessionAgentBuilder + 'static>,
    capability_provider: &LiveCapabilityProvider,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    #[cfg(feature = "openai-live")]
    let _ = (ctx, machine);
    let parsed: LiveChannelParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);

    #[cfg(feature = "openai-live")]
    {
        let status = match shared_live_host
            .close_live_channel(capability_provider.open_authority(), &channel_id)
            .await
        {
            Ok(status) => status,
            Err(meerkat::surface::ExperimentalLiveChannelCloseError::BindingMismatch)
                if live_channel_close_already_complete(machine, &channel_id).await =>
            {
                // The channel completed its close earlier: the machine holds
                // only its close status and no active binding. Closing it
                // again is a no-op that succeeds, so a caller retrying after
                // a dropped response or a dead remote converges instead of
                // failing forever with a binding error.
                meerkat_contracts::LiveCloseStatus::Closed
            }
            Err(error) => {
                return live_error(rpc_id, live_close_error_code(&error), error.to_string());
            }
        };
        match serde_json::to_value(LiveCloseResult { status }) {
            Ok(body) => live_success(rpc_id, body),
            Err(error) => live_error(
                rpc_id,
                INTERNAL_ERROR_CODE,
                format!("live close authority projection failed: {error}"),
            ),
        }
    }

    #[cfg(not(feature = "openai-live"))]
    {
        let _ = (shared_live_host, capability_provider);

        let request_kind =
            meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::Close;
        let Some(session_id) = machine.live_session_for_active_channel(&channel_id).await else {
            return live_unbound_channel_request_error_response(
                rpc_id,
                machine,
                &channel_id,
                request_kind,
            )
            .await;
        };

        match ctx
            .host
            .reserve_channel_close_observation(
                &channel_id,
                meerkat_core::LiveChannelCloseReason::ClientRequested,
            )
            .await
        {
            Ok(observation) => {
                // Reference order (`close_live_channel` in the facade's live
                // orchestration): reserve -> PHYSICAL close -> generated close
                // authority -> host commit. The host commit fails closed
                // (`CloseNotAuthorized`) unless the adapter was physically
                // closed and detached first; the original port omitted this
                // step, so every RPC-initiated close of an attached channel
                // died typed instead of closing.
                if let Err(error) = ctx.host.prepare_channel_physical_close(&observation).await {
                    return live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!(
                            "physical adapter close failed before generated terminal authority: {error}"
                        ),
                    );
                }
                let authority = match machine
                    .resolve_live_close_result(&session_id, &observation)
                    .await
                {
                    Ok(authority) => authority,
                    Err(error) => {
                        return live_error(
                            rpc_id,
                            INTERNAL_ERROR_CODE,
                            format!("live close authority rejected result: {error}"),
                        );
                    }
                };
                let Some(close_commit_authority) = authority.channel_close_commit_authority()
                else {
                    return live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        "live close authority omitted host commit handoff",
                    );
                };
                if let Err(error) = ctx
                    .host
                    .commit_channel_close_observation(&observation, close_commit_authority)
                    .await
                {
                    return live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!("live close host commit failed after generated authority: {error}"),
                    );
                }
                let result = live_close_result_from_machine_authority(&authority);
                match serde_json::to_value(result) {
                    Ok(body) => live_success(rpc_id, body),
                    Err(error) => live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!("live close authority projection failed: {error}"),
                    ),
                }
            }
            Err(err) => {
                live_channel_request_error_response(
                    rpc_id,
                    machine,
                    &session_id,
                    &channel_id,
                    request_kind,
                    &err,
                )
                .await
            }
        }
    }
}

/// P1#5: enqueue a mutable live-config update against the active channel.
/// Configuration only: history is `live/open`'s seed step (re-seeding on
/// refresh compounds the provider transcript), so the refresh projection comes
/// from the hydration-free refresh export, takes no open reservation and
/// carries no seed history. The channel keeps the realtime identity its open
/// bound; identity swaps require close and reopen. R7: the honest reply is
/// `status: queued`; the adapter pump applies asynchronously and failures
/// surface on the realtime stream.
async fn handle_live_refresh<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    service: &Arc<PersistentSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: LiveChannelParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);

    let request_kind = meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind::Refresh;
    let Some(session_id) = machine.live_session_for_active_channel(&channel_id).await else {
        return live_unbound_channel_request_error_response(
            rpc_id,
            machine,
            &channel_id,
            request_kind,
        )
        .await;
    };

    // The realtime identity the channel's open bound, from its native owner;
    // never the member's text identity.
    let llm_identity = match live_refresh_channel_identity(
        machine,
        &session_id,
        &channel_id,
        rpc_id.clone(),
    )
    .await
    {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let refresh_config =
        match live_refresh_config_for_session(service, &session_id, llm_identity).await {
            Ok(config) => config,
            Err(err) => {
                return live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!("failed to build session config: {err}"),
                );
            }
        };
    // R8: stamp the host's monotonic per-channel version so adapters gating
    // on `snapshot_version` for stale-refresh detection see strictly
    // increasing generations. Refresh carries no audio policy; the format
    // negotiated by the shared open pipeline stays in force.
    #[cfg(feature = "openai-live")]
    let mut snapshot = build_live_projection_snapshot(&session_id, &refresh_config);
    #[cfg(not(feature = "openai-live"))]
    let mut snapshot = build_live_projection_snapshot(&session_id, &refresh_config, None);
    match ctx.host.next_snapshot_version(&channel_id).await {
        Ok(v) => snapshot.snapshot_version = v,
        Err(err) => {
            return live_channel_request_error_response(
                rpc_id,
                machine,
                &session_id,
                &channel_id,
                request_kind,
                &err,
            )
            .await;
        }
    }

    match ctx.host.enqueue_refresh(&channel_id, snapshot).await {
        Ok(acceptance) => {
            let authority = match machine
                .resolve_live_refresh_queued_result(&session_id, &acceptance)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!("live refresh queued authority rejected result: {error}"),
                    );
                }
            };
            let result = live_refresh_result_from_machine_authority(&authority);
            match serde_json::to_value(result) {
                Ok(body) => live_success(rpc_id, body),
                Err(error) => live_error(
                    rpc_id,
                    INTERNAL_ERROR_CODE,
                    format!("live refresh queued authority projection failed: {error}"),
                ),
            }
        }
        Err(err) => {
            live_channel_request_error_response(
                rpc_id,
                machine,
                &session_id,
                &channel_id,
                request_kind,
                &err,
            )
            .await
        }
    }
}

async fn handle_live_send_input(
    ctx: &GatewayLiveContext,
    machine: &Arc<MeerkatMachine>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: LiveSendInputParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);
    let chunk = match live_input_chunk_from_wire(parsed.chunk) {
        Ok(chunk) => chunk,
        Err(err) => {
            return live_error(rpc_id, INVALID_PARAMS_CODE, err.to_string());
        }
    };

    let command_kind = meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind::SendInput;
    let Some(session_id) = machine.live_session_for_active_channel(&channel_id).await else {
        return live_unbound_command_error_response(rpc_id, machine, &channel_id, command_kind)
            .await;
    };

    match ctx.host.send_input_observed(&channel_id, chunk).await {
        Ok(acceptance) => {
            let authority = match machine
                .resolve_live_command_result(&session_id, &acceptance)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!("live send_input authority rejected result: {error}"),
                    );
                }
            };
            match live_command_result_from_machine_authority(&authority, command_kind) {
                Ok(value) => live_success(rpc_id, value),
                Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error),
            }
        }
        Err(err) => {
            live_command_error_response(
                rpc_id,
                machine,
                &session_id,
                &channel_id,
                command_kind,
                &err,
            )
            .await
        }
    }
}

/// I50/G9: flush buffered uncommitted input; the optional
/// `response_modality` requests a text-only response on an audio-first
/// channel without flipping the channel-wide modality.
async fn handle_live_commit_input(
    ctx: &GatewayLiveContext,
    machine: &Arc<MeerkatMachine>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: LiveCommitInputParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);
    // R5-3: the wire mirror is `TryFrom`-only; an unrecognized future
    // modality rejects loudly instead of silently coercing.
    let response_modality = match parsed.response_modality.map(TryInto::try_into) {
        Some(Ok(modality)) => Some(modality),
        Some(Err(err)) => {
            return live_error(
                rpc_id,
                INVALID_PARAMS_CODE,
                format!("invalid response_modality: {err}"),
            );
        }
        None => None,
    };

    let command_kind = meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind::CommitInput;
    let Some(session_id) = machine.live_session_for_active_channel(&channel_id).await else {
        return live_unbound_command_error_response(rpc_id, machine, &channel_id, command_kind)
            .await;
    };

    match ctx
        .host
        .send_command_observed(
            &channel_id,
            LiveAdapterCommand::CommitInput { response_modality },
        )
        .await
    {
        Ok(acceptance) => {
            let authority = match machine
                .resolve_live_command_result(&session_id, &acceptance)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!("live commit_input authority rejected result: {error}"),
                    );
                }
            };
            match live_command_result_from_machine_authority(&authority, command_kind) {
                Ok(value) => live_success(rpc_id, value),
                Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error),
            }
        }
        Err(err) => {
            live_command_error_response(
                rpc_id,
                machine,
                &session_id,
                &channel_id,
                command_kind,
                &err,
            )
            .await
        }
    }
}

/// A7: explicit barge-in surface - without it callers can only rely on
/// provider-native VAD.
async fn handle_live_interrupt(
    ctx: &GatewayLiveContext,
    machine: &Arc<MeerkatMachine>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: LiveChannelParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);
    let command_kind = meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind::Interrupt;
    let Some(session_id) = machine.live_session_for_active_channel(&channel_id).await else {
        return live_unbound_command_error_response(rpc_id, machine, &channel_id, command_kind)
            .await;
    };

    match ctx
        .host
        .send_command_observed(&channel_id, LiveAdapterCommand::Interrupt)
        .await
    {
        Ok(acceptance) => {
            let authority = match machine
                .resolve_live_command_result(&session_id, &acceptance)
                .await
            {
                Ok(authority) => authority,
                Err(error) => {
                    return live_error(
                        rpc_id,
                        INTERNAL_ERROR_CODE,
                        format!("live interrupt authority rejected result: {error}"),
                    );
                }
            };
            match live_command_result_from_machine_authority(&authority, command_kind) {
                Ok(value) => live_success(rpc_id, value),
                Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error),
            }
        }
        Err(err) => {
            live_command_error_response(
                rpc_id,
                machine,
                &session_id,
                &channel_id,
                command_kind,
                &err,
            )
            .await
        }
    }
}

#[cfg(not(feature = "openai-live"))]
/// A7: `meta/live/truncate` - truncate an assistant item at the given
/// playback cursor, through Meerkat's OWNER SEAM.
///
/// This used to construct `LiveAdapterCommand::TruncateAssistantOutput`
/// directly. At Meerkat 0.8.30 that command requires an `InteractionId` which
/// is "resolved from the session's admitted playback target, never minted by a
/// caller or surface", so a downstream surface structurally cannot build it, by
/// design. `ServiceMemberLiveHost::truncate_stock_live_output` delegates to the
/// canonical `LiveOrchestrator` with the legacy item/content cursor and exposes
/// no interaction argument - which is why the whole command/acceptance/authority
/// round trip collapses into one call here.
async fn handle_live_truncate<B: SessionAgentBuilder + 'static>(
    ctx: &GatewayLiveContext,
    service: &Arc<PersistentSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: LiveTruncateParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    // 0.8.30 made `item_id` and `content_index` optional on the wire because
    // EXPERIMENTAL channels address output by opaque `output_id` instead. The
    // stock path still requires the provider address, and a missing one is a
    // typed refusal rather than a fabricated empty value - the same rule the
    // transcript path already applies: never fabricate empty identity.
    let Some(item_id) = parsed
        .item_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return live_error(
            rpc_id,
            INVALID_PARAMS_CODE,
            "item_id must be a non-empty provider item address on the stock live path",
        );
    };
    // Deliberately NOT defaulted to 0: truncating content index 0 when the
    // caller meant another part would silently discard the wrong content.
    let Some(content_index) = parsed.content_index else {
        return live_error(
            rpc_id,
            INVALID_PARAMS_CODE,
            "content_index is required on the stock live truncate path",
        );
    };

    let channel_id = LiveChannelId::new(&parsed.channel_id);
    let host = shared_live_host(ctx, service, machine);
    match host
        .truncate_stock_live_output(
            &channel_id,
            item_id,
            content_index,
            parsed.audio_played_ms,
            // Forwarded UNCHANGED. Omission is explicit `Unmeasured` coverage
            // and authorizes no canonical assistant transcript replacement, so
            // it must never be synthesized from audio_played_ms.
            parsed.reported_playback_prefix,
        )
        .await
    {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => live_success(rpc_id, value),
            Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
        },
        Err(error) => live_error(rpc_id, INVALID_PARAMS_CODE, error.to_string()),
    }
}

/// Truncate one exact machine-sealed assistant output at the playback
/// owner's measured cursor. Meta accepts no provider item or caller-minted
/// interaction identity on this boundary.
#[cfg(feature = "openai-live")]
async fn handle_live_truncate<B: SessionAgentBuilder + 'static>(
    shared_live_host: &ServiceMemberLiveHost<B>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: LiveTruncateParams = match parse_live_params(params, &rpc_id) {
        Ok(p) => p,
        Err(resp) => return *resp,
    };
    if parsed.item_id.is_some() || parsed.content_index.is_some() {
        return live_error(
            rpc_id,
            INVALID_PARAMS_CODE,
            "meta live truncate accepts only an opaque output_id",
        );
    }
    let Some(output_id) = parsed
        .output_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return live_error(rpc_id, INVALID_PARAMS_CODE, "output_id must be non-empty");
    };
    let channel_id = LiveChannelId::new(&parsed.channel_id);
    let Some(activation_receipt) = params.get("activation_receipt").and_then(Value::as_str) else {
        return live_error(
            rpc_id,
            INVALID_PARAMS_CODE,
            "strict live truncate requires activation_receipt",
        );
    };
    match shared_live_host
        .truncate_live_output(
            &channel_id,
            activation_receipt,
            output_id,
            parsed.audio_played_ms,
            parsed.reported_playback_prefix,
        )
        .await
    {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => live_success(rpc_id, value),
            Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
        },
        Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
    }
}

#[cfg(feature = "openai-live")]
async fn handle_live_playback_complete<B: SessionAgentBuilder + 'static>(
    shared_live_host: &ServiceMemberLiveHost<B>,
    params: &Value,
    rpc_id: Value,
) -> JsonRpcResponse {
    let parsed: LivePlaybackCompleteParams = match parse_live_params(params, &rpc_id) {
        Ok(parsed) => parsed,
        Err(response) => return *response,
    };
    if parsed.output_id.trim().is_empty() {
        return live_error(rpc_id, INVALID_PARAMS_CODE, "output_id must be non-empty");
    }
    let channel_id = LiveChannelId::new(&parsed.channel_id);
    let Some(activation_receipt) = params.get("activation_receipt").and_then(Value::as_str) else {
        return live_error(
            rpc_id,
            INVALID_PARAMS_CODE,
            "strict live playback completion requires activation_receipt",
        );
    };
    match shared_live_host
        .complete_live_playback(&channel_id, activation_receipt, &parsed.output_id)
        .await
    {
        Ok(result) => match serde_json::to_value(result) {
            Ok(value) => live_success(rpc_id, value),
            Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
        },
        Err(error) => live_error(rpc_id, INTERNAL_ERROR_CODE, error.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[cfg(feature = "openai-live")]
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(feature = "openai-live-test")]
    use std::sync::atomic::AtomicBool;

    #[cfg(feature = "openai-live-test")]
    use meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator as _;

    /// A live open refused because another live open holds Meerkat's
    /// one-at-a-time open admission answers `-32054` with the typed,
    /// retryable `live_open_busy` payload, not an internal error.
    #[cfg(feature = "openai-live")]
    #[test]
    fn a_backpressured_live_open_answers_the_retryable_live_open_busy_payload() {
        let response = live_open_error_response(
            serde_json::json!(7),
            LiveOpenError::OpenConfig(RealtimeSessionOpenProjectionError::Backpressured),
        );
        let error = response.error.expect("a busy open is refused");
        assert_eq!(error.code, LIVE_OPEN_BUSY_CODE);
        assert_eq!(
            error.data,
            Some(serde_json::json!({"kind": LIVE_OPEN_BUSY_KIND, "retryable": true}))
        );
    }

    #[test]
    fn ordinary_live_instruction_overlay_preserves_legacy_normalization() {
        assert_eq!(
            live_open_instruction_overlay(vec![
                "  Speak as the room embodiment.  ".to_string(),
                "".to_string(),
                "Keep replies concise.".to_string(),
            ]),
            Some("Speak as the room embodiment.\n\nKeep replies concise.".to_string())
        );
        assert_eq!(
            live_open_instruction_overlay(vec!["  ".to_string(), "\n".to_string()]),
            None
        );
    }

    #[test]
    fn stock_live_capability_provider_is_fail_closed() {
        assert!(
            LiveCapabilityProvider::disabled()
                .feature_capabilities()
                .is_empty()
        );
        assert!(
            LiveCapabilityProvider::default()
                .feature_capabilities()
                .is_empty()
        );
        #[cfg(feature = "openai-live")]
        assert!(
            LiveCapabilityProvider::disabled()
                .bound_ready_binder()
                .is_none()
        );
    }

    #[cfg(not(feature = "openai-live"))]
    #[test]
    fn published_meerkat_compatibility_keeps_only_the_ordinary_live_contract() {
        assert_eq!(
            LiveOperation::from_method("meta/live/truncate"),
            Some(LiveOperation::Truncate),
        );
        assert!(
            LiveOperation::from_method("live/webrtc/answer").is_none(),
            "the experimental answer method must remain absent on the stock 0.8.26 path",
        );
        assert!(
            LiveCapabilityProvider::disabled()
                .feature_capabilities()
                .is_empty()
        );
    }

    /// The ordinary open delivers the staged prompt at the session's turn
    /// boundary, as Meerkat's ordinary open does: while a holder other than a
    /// turn has the boundary the delivery waits and commits nothing, and once
    /// the boundary is free it commits the prompt once. A later delivery finds
    /// nothing staged and commits no second row.
    #[cfg(not(feature = "openai-live"))]
    #[tokio::test]
    async fn a_held_turn_boundary_delays_the_ordinary_open_delivery_which_delivers_once() {
        use meerkat_core::service::SessionService as _;

        const STAGED: &str = "Remember the codeword ORBIT-7 for later.";
        let persistence = meerkat::PersistenceBundle::new(
            Arc::new(meerkat::MemoryStore::new()),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("staged-prompt fixture persistence");
        let temp = tempfile::tempdir().expect("staged-prompt fixture state");
        let mut builder = meerkat::FactoryAgentBuilder::new(
            AgentFactory::new(temp.path()).builtins(false),
            Config::default(),
        );
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let (service, machine) =
            meerkat::surface::build_runtime_backed_service(builder, 4, persistence)
                .expect("compose the runtime-backed service");
        let session = meerkat_core::Session::new();
        let session_id = session.id().clone();
        let bindings = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare runtime bindings");
        service
            .create_session(meerkat_core::service::CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: meerkat_core::ContentInput::Text(STAGED.to_string()),
                injected_context: Vec::new(),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Stage,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    resume_session: Some(session),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    ..meerkat_core::service::SessionBuildOptions::default()
                }),
                labels: None,
            })
            .await
            .expect("create the deferred session with its prompt staged");

        // Another holder of the turn boundary (not a turn: nothing consumed
        // the prompt). The delivery waits for it without committing anything.
        let boundary = service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await;
        let mut delivery =
            std::pin::pin!(deliver_staged_prompt_for_live_open(&service, &session_id));
        let while_held =
            tokio::time::timeout(std::time::Duration::from_millis(250), delivery.as_mut()).await;
        let rows_while_held = committed_user_rows(&service, &session_id).await;
        drop(boundary);
        let refused = tokio::time::timeout(std::time::Duration::from_secs(20), delivery.as_mut())
            .await
            .expect("the delivery completes once the boundary is free")
            .expect("the delivery answers");
        let rows = committed_user_rows(&service, &session_id).await;
        let refused_again = deliver_staged_prompt_for_live_open(&service, &session_id)
            .await
            .expect("a later delivery answers");
        let rows_after_again = committed_user_rows(&service, &session_id).await;
        service.shutdown().await;

        assert!(
            while_held.is_err()
                && rows_while_held.is_empty()
                && !refused
                && rows == [STAGED.to_string()]
                && !refused_again
                && rows_after_again == [STAGED.to_string()],
            "a held boundary must delay the delivery without committing, and the delivery \
             must then commit the prompt once: while_held={while_held:?} \
             rows_while_held={rows_while_held:?} refused={refused} rows={rows:?} \
             refused_again={refused_again} rows_after_again={rows_after_again:?}"
        );
    }

    /// The text of every committed user row of `session_id`'s transcript.
    #[cfg(not(feature = "openai-live"))]
    async fn committed_user_rows<B: SessionAgentBuilder + 'static>(
        service: &PersistentSessionService<B>,
        session_id: &SessionId,
    ) -> Vec<String> {
        service
            .load_authoritative_session(session_id)
            .await
            .expect("load the committed session")
            .map(|session| {
                session
                    .messages()
                    .iter()
                    .filter_map(|message| match message {
                        meerkat_core::Message::User(user) => Some(user.text_content()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(feature = "openai-live")]
    struct StaticSessionOwner(bool);

    #[cfg(feature = "openai-live")]
    struct NoBinderOpenAuthority;

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider
        for NoBinderOpenAuthority
    {
        async fn prepare_open(
            &self,
            _canonical_session_id: &SessionId,
            _execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
        ) -> Result<
            Box<dyn meerkat::experimental_gpt_live::ExperimentalLivePendingOpen>,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
        > {
            Err(meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::Unavailable)
        }

        async fn unbind_channel(
            &self,
            _channel_id: &LiveChannelId,
            _canonical_session_id: &SessionId,
        ) {
        }

        async fn close_physical_if_bound(
            &self,
            _channel_id: &LiveChannelId,
            _canonical_session_id: &SessionId,
        ) -> Result<
            meerkat::experimental_gpt_live::ExperimentalLivePhysicalClose,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
        > {
            Ok(meerkat::experimental_gpt_live::ExperimentalLivePhysicalClose::NotBound)
        }
    }

    /// Records the post-close work source a composition binds.
    #[cfg(feature = "openai-live-test")]
    #[derive(Default)]
    struct RecordingPostCloseAuthority {
        bound: std::sync::Mutex<
            Option<Arc<dyn meerkat::experimental_gpt_live::LivePostCloseWorkSource>>,
        >,
    }

    #[cfg(feature = "openai-live-test")]
    #[async_trait]
    impl meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider
        for RecordingPostCloseAuthority
    {
        async fn prepare_open(
            &self,
            _canonical_session_id: &SessionId,
            _execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
        ) -> Result<
            Box<dyn meerkat::experimental_gpt_live::ExperimentalLivePendingOpen>,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
        > {
            Err(meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::Unavailable)
        }

        fn bind_post_close_work_source(
            &self,
            source: Arc<dyn meerkat::experimental_gpt_live::LivePostCloseWorkSource>,
        ) {
            let mut bound = self
                .bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if bound.is_none() {
                *bound = Some(source);
            }
        }

        async fn unbind_channel(
            &self,
            _channel_id: &LiveChannelId,
            _canonical_session_id: &SessionId,
        ) {
        }

        async fn close_physical_if_bound(
            &self,
            _channel_id: &LiveChannelId,
            _canonical_session_id: &SessionId,
        ) -> Result<
            meerkat::experimental_gpt_live::ExperimentalLivePhysicalClose,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
        > {
            Ok(meerkat::experimental_gpt_live::ExperimentalLivePhysicalClose::NotBound)
        }
    }

    /// meerkat #1652: composing a live door binds its delegation coordinator
    /// as the open authority's post-close work source, so a reopened channel
    /// can say that earlier work is still finishing. Without it only
    /// meerkat's RPC router bound one, and Meta's doors never did.
    #[cfg(feature = "openai-live-test")]
    #[tokio::test]
    async fn composing_a_live_door_binds_the_post_close_work_source() {
        let persistence = meerkat::PersistenceBundle::new(
            Arc::new(meerkat::MemoryStore::new()),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("post-close fixture persistence");
        let temp = tempfile::tempdir().expect("post-close fixture state");
        let factory = AgentFactory::new(temp.path()).builtins(false);
        let mut builder = meerkat::FactoryAgentBuilder::new(factory.clone(), Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let (service, machine) =
            meerkat::surface::build_runtime_backed_service(builder, 4, persistence)
                .expect("compose the runtime-backed service");
        let service = Arc::new(service);
        let ctx = attach_live(
            Arc::clone(&service),
            Arc::clone(&machine),
            &factory,
            Config::default(),
            "ws://127.0.0.1/post-close".to_string(),
            None,
        );
        let authority = Arc::new(RecordingPostCloseAuthority::default());
        let provider = LiveCapabilityProvider {
            configured: Some(Arc::new(ConfiguredLiveCapabilityProvider {
                factory: Arc::new(AgentFactory::minimal()),
                realm: RealmId::parse("mob.example").expect("realm"),
                experimental_factory: None,
                open_authority: Arc::clone(&authority) as _,
                answer_transport: Arc::new(UnusedAnswerTransport),
                public_observation_publisher: Arc::new(UnusedPublicObservationPublisher),
                activator: ExperimentalLiveActivatorRegistration::Uncomposed(
                    meerkat_mob_mcp::MobMcpState::new_in_memory(),
                ),
                live_adapter_host: None,
                phase_authority_composed: false,
            })),
        };
        let _composed = provider.compose_for_host(
            Arc::clone(&machine),
            Arc::new(shared_live_host(&ctx, &service, &machine)),
            Arc::clone(&ctx.host),
            meerkat_mob_mcp::live_delegation::LiveDelegationExecutionPolicy::default(),
        );
        let source = authority
            .bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("the composition binds its coordinator as the post-close work source");
        assert!(
            !source.has_undelivered_post_close_result(&SessionId::new()),
            "a fresh coordinator reports no work outliving a channel"
        );
    }

    #[cfg(feature = "openai-live")]
    struct UnusedAnswerTransport;

    #[cfg(feature = "openai-live")]
    struct UnusedPublicObservationPublisher;

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat::experimental_gpt_live::ExperimentalLivePublicObservationPublisher
        for UnusedPublicObservationPublisher
    {
        async fn publish(
            &self,
            _observation: meerkat::experimental_gpt_live::ExperimentalLivePublicObservation,
        ) -> Result<
            (),
            meerkat::experimental_gpt_live::ExperimentalLivePublicObservationDeliveryError,
        > {
            Ok(())
        }
    }

    #[cfg(feature = "openai-live")]
    struct UnusedBoundChannelActivator;

    #[cfg(feature = "openai-live")]
    struct PendingReplacementActivator {
        reads: AtomicUsize,
        pending: meerkat::surface::ExperimentalLiveReplacementRequired,
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator
        for UnusedBoundChannelActivator
    {
        async fn prepare_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn meerkat::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        ) -> Result<(), String> {
            Err("unused".to_string())
        }

        async fn run_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn meerkat::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        ) {
        }

        async fn observe_provider_lifecycle(
            &self,
            _observation: &meerkat_live::LiveSidebandObservation,
        ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveLifecycleObservationError>
        {
            Ok(())
        }

        async fn deactivate_bound_channel(
            &self,
            _binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator
        for PendingReplacementActivator
    {
        async fn prepare_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn meerkat::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn run_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn meerkat::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        ) {
        }

        async fn observe_provider_lifecycle(
            &self,
            _observation: &meerkat_live::LiveSidebandObservation,
        ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveLifecycleObservationError>
        {
            Ok(())
        }

        async fn deactivate_bound_channel(
            &self,
            _binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn pending_replacement_required(
            &self,
            _session_id: &SessionId,
        ) -> Option<meerkat::surface::ExperimentalLiveReplacementRequired> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Some(self.pending.clone())
        }
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat_live::LiveWebrtcAnswerTransport for UnusedAnswerTransport {
        async fn answer_admitted_offer(
            &self,
            _offer: meerkat_live::LiveWebrtcAdmittedOffer,
        ) -> Result<meerkat_live::LiveWebrtcAnswerAccepted, meerkat_live::LiveWebrtcError> {
            Err(meerkat_live::LiveWebrtcError::ChannelNotFound(
                "unused".to_string(),
            ))
        }

        async fn reject_answer(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
            _answer_observation_sequence: u64,
        ) -> Result<(), meerkat_live::LiveWebrtcError> {
            Ok(())
        }

        async fn accept_answer(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
            _answer_observation_sequence: u64,
        ) {
        }

        async fn wait_for_construction_cleanup(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
        ) -> Result<(), meerkat_live::LiveWebrtcError> {
            Ok(())
        }

        async fn close_binding(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
        ) -> Result<(), meerkat_live::LiveWebrtcError> {
            Ok(())
        }
    }

    #[cfg(feature = "openai-live-test")]
    struct ReceiptSideband;

    #[cfg(feature = "openai-live-test")]
    #[async_trait]
    impl meerkat_live::ProviderWebrtcSidebandSession for ReceiptSideband {
        async fn send_command(
            &self,
            _command: meerkat_live::LiveSidebandCommand,
        ) -> Result<
            meerkat_live::LiveSidebandCommandDelivery,
            meerkat_live::ProviderWebrtcBrokerError,
        > {
            Err(meerkat_live::ProviderWebrtcBrokerError::Unavailable)
        }

        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_live::LiveSidebandObservation>,
            meerkat_live::ProviderWebrtcBrokerError,
        > {
            Ok(None)
        }

        async fn close(&self) -> Result<(), meerkat_live::ProviderWebrtcBrokerError> {
            Ok(())
        }
    }

    #[cfg(feature = "openai-live-test")]
    struct ReceiptAnswerTransport {
        accepted: AtomicUsize,
        rejected: AtomicUsize,
    }

    #[cfg(feature = "openai-live-test")]
    struct ReceiptPendingBoundReady;

    #[cfg(feature = "openai-live-test")]
    #[async_trait]
    impl meerkat_live::ProviderWebrtcPendingBoundReadyResolver for ReceiptPendingBoundReady {
        async fn resolve(self: Box<Self>) -> Result<u64, meerkat_live::ProviderWebrtcBrokerError> {
            Ok(0)
        }
    }

    #[cfg(feature = "openai-live-test")]
    #[async_trait]
    impl meerkat_live::LiveWebrtcAnswerTransport for ReceiptAnswerTransport {
        async fn answer_admitted_offer(
            &self,
            offer: meerkat_live::LiveWebrtcAdmittedOffer,
        ) -> Result<meerkat_live::LiveWebrtcAnswerAccepted, meerkat_live::LiveWebrtcError> {
            let answer = offer
                .into_provider_offer()?
                .into_pending_bound_ready_answer(
                    "receipt-answer".to_string(),
                    Arc::new(ReceiptSideband),
                    Box::new(ReceiptPendingBoundReady),
                );
            let (answer_sdp, _sideband, pending_bound_ready) = answer.into_parts();
            Ok(meerkat_live::LiveWebrtcAnswerAccepted {
                answer_sdp,
                answer_observation_sequence: 41,
                pending_bound_ready: Some(pending_bound_ready),
            })
        }

        async fn reject_answer(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
            _answer_observation_sequence: u64,
        ) -> Result<(), meerkat_live::LiveWebrtcError> {
            self.rejected.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn accept_answer(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
            _answer_observation_sequence: u64,
        ) {
            self.accepted.fetch_add(1, Ordering::SeqCst);
        }

        async fn wait_for_construction_cleanup(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
        ) -> Result<(), meerkat_live::LiveWebrtcError> {
            Ok(())
        }

        async fn close_binding(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
        ) -> Result<(), meerkat_live::LiveWebrtcError> {
            Ok(())
        }
    }

    #[cfg(feature = "openai-live-test")]
    struct TestControlPlane {
        binding: meerkat_live::ProviderWebrtcBinding,
    }

    #[cfg(feature = "openai-live-test")]
    #[async_trait]
    impl meerkat::experimental_gpt_live::ExperimentalGptLiveControlPlane for TestControlPlane {
        async fn active_binding(
            &self,
            session_id: &SessionId,
        ) -> Option<meerkat_live::ProviderWebrtcBinding> {
            (self.binding.session_id() == session_id).then(|| self.binding.clone())
        }

        async fn next_observation(
            &self,
            _binding: &meerkat_live::ProviderWebrtcBinding,
        ) -> Result<
            Option<meerkat::experimental_gpt_live::ExperimentalGptLiveControlObservation>,
            meerkat_live::ProviderWebrtcBrokerError,
        > {
            Ok(None)
        }

        async fn append_session_context(
            &self,
            _authority: meerkat_runtime::live_execution::LiveContextAppendAuthority,
            _text: String,
        ) -> Result<
            meerkat::experimental_gpt_live::ExperimentalGptLiveAppendDispatch,
            meerkat::experimental_gpt_live::ExperimentalGptLiveBridgeError,
        > {
            Err(meerkat::experimental_gpt_live::ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
        }

        async fn release_delegation_context(
            &self,
            _authority: meerkat_runtime::live_execution::LiveDelegationResultDeliveryAuthority,
            _delegation: meerkat_live::LiveSidebandDelegationRef,
            _text: String,
        ) -> Result<
            meerkat::experimental_gpt_live::ExperimentalGptLiveResultDeliveryDispatch,
            meerkat::experimental_gpt_live::ExperimentalGptLiveBridgeError,
        > {
            Err(meerkat::experimental_gpt_live::ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
        }

        async fn narrate_delegation(
            &self,
            _authority: meerkat_runtime::live_execution::LiveDelegationNarrationAuthority,
            _delegation: meerkat_live::LiveSidebandDelegationRef,
            _text: String,
        ) -> Result<
            meerkat::experimental_gpt_live::ExperimentalGptLiveNarrationDispatch,
            meerkat::experimental_gpt_live::ExperimentalGptLiveBridgeError,
        > {
            Err(meerkat::experimental_gpt_live::ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
        }
    }

    #[cfg(feature = "openai-live-test")]
    struct OrderingActivator {
        machine_bound: Arc<AtomicBool>,
        calls: AtomicUsize,
    }

    #[cfg(feature = "openai-live-test")]
    #[async_trait]
    impl meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator for OrderingActivator {
        async fn prepare_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn meerkat::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        ) -> Result<(), String> {
            if !self.machine_bound.load(Ordering::SeqCst) {
                return Err("activator ran before atomic machine binding".to_string());
            }
            Ok(())
        }

        async fn run_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn meerkat::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        ) {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }

        async fn observe_provider_lifecycle(
            &self,
            _observation: &meerkat_live::LiveSidebandObservation,
        ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveLifecycleObservationError>
        {
            Ok(())
        }

        async fn deactivate_bound_channel(
            &self,
            _binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[cfg(feature = "openai-live-test")]
    struct TestBoundReadyCustody {
        authority:
            Option<meerkat_runtime::meerkat_machine::LiveWebrtcAnswerExecutionBindingAuthority>,
        activator: Arc<OrderingActivator>,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn meerkat::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        committed: Arc<AtomicBool>,
        rolled_back: Arc<AtomicBool>,
    }

    #[cfg(feature = "openai-live-test")]
    #[async_trait]
    impl meerkat::surface::LiveWebrtcBoundReadyCustody for TestBoundReadyCustody {
        async fn commit(mut self: Box<Self>) -> Result<(), String> {
            if let Some(authority) = self.authority.take() {
                let _ = authority.commit();
            }
            self.committed.store(true, Ordering::SeqCst);
            self.activator
                .run_bound_channel(self.binding.clone(), Arc::clone(&self.control))
                .await;
            Ok(())
        }

        async fn rollback(mut self: Box<Self>) -> Result<(), String> {
            let _rollback = self
                .authority
                .take()
                .map(
                    meerkat_runtime::meerkat_machine::LiveWebrtcAnswerExecutionBindingAuthority::into_rollback,
                );
            self.activator
                .deactivate_bound_channel(&self.binding)
                .await?;
            self.rolled_back.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[cfg(feature = "openai-live-test")]
    struct OrderingBinder {
        activator: Arc<OrderingActivator>,
        machine_bound: Arc<AtomicBool>,
        committed: Arc<AtomicBool>,
        rolled_back: Arc<AtomicBool>,
    }

    #[cfg(feature = "openai-live-test")]
    #[async_trait]
    impl meerkat::surface::LiveWebrtcBoundReadyBinder for OrderingBinder {
        async fn bind_answer_ready(
            &self,
            runtime: Arc<MeerkatMachine>,
            binding: &meerkat_live::LiveWebrtcBindingRequest,
            receipt: meerkat_live::ProviderWebrtcBoundReadyReceipt,
            answer_observation_sequence: u64,
        ) -> Result<
            Box<dyn meerkat::surface::LiveWebrtcBoundReadyCustody>,
            meerkat::surface::LiveWebrtcBoundReadyBindFailure,
        > {
            let runtime_binding = binding
                .runtime_binding
                .expect("staged experimental channel has runtime binding");
            let provider_binding = meerkat_live::ProviderWebrtcBinding::new(
                binding.channel_id.clone(),
                binding.session_id.clone(),
                meerkat_live::LiveRuntimeBindingGeneration::new(runtime_binding.generation),
                meerkat_live::LiveRuntimeBindingFence::new(runtime_binding.fence),
            );
            let authority = runtime
                .accept_live_webrtc_answer_and_bind_execution(
                    &provider_binding,
                    &receipt,
                    answer_observation_sequence,
                )
                .await
                .expect("atomic answer and execution bind");
            self.machine_bound.store(true, Ordering::SeqCst);
            let binding = authority.binding().clone();
            let control: Arc<dyn meerkat::experimental_gpt_live::ExperimentalGptLiveControlPlane> =
                Arc::new(TestControlPlane {
                    binding: provider_binding,
                });
            self.activator
                .prepare_bound_channel(binding.clone(), Arc::clone(&control))
                .await
                .expect("post-bind activation preparation");
            Ok(Box::new(TestBoundReadyCustody {
                authority: Some(authority),
                activator: Arc::clone(&self.activator),
                binding,
                control,
                committed: Arc::clone(&self.committed),
                rolled_back: Arc::clone(&self.rolled_back),
            }))
        }
    }

    #[cfg(feature = "openai-live")]
    #[test]
    fn configured_registration_without_authority_binder_does_not_advertise() {
        let provider = LiveCapabilityProvider {
            configured: Some(Arc::new(ConfiguredLiveCapabilityProvider {
                factory: Arc::new(AgentFactory::minimal()),
                realm: RealmId::parse("mob.example").expect("realm"),
                experimental_factory: None,
                open_authority: Arc::new(NoBinderOpenAuthority),
                answer_transport: Arc::new(UnusedAnswerTransport),
                public_observation_publisher: Arc::new(UnusedPublicObservationPublisher),
                activator: ExperimentalLiveActivatorRegistration::Composed(Arc::new(
                    UnusedBoundChannelActivator,
                )),
                live_adapter_host: None,
                phase_authority_composed: false,
            })),
        };

        assert!(provider.feature_capabilities().is_empty());
        assert!(provider.bound_ready_binder().is_none());
    }

    #[cfg(feature = "openai-live")]
    fn replacement_open_result(channel_id: &str) -> meerkat_contracts::LiveOpenResult {
        serde_json::from_value(serde_json::json!({
            "channel_id": channel_id,
            "transport": {
                "transport": "webrtc",
                "token": "fresh-token",
                "answer_method": "live/webrtc/answer"
            },
            "capabilities": {
                "audio_in": true,
                "audio_out": true,
                "text_in": true,
                "text_out": true,
                "image_in": false,
                "video_in": false,
                "transcript_supported": true,
                "barge_in_supported": true,
                "provider_native_resume": false
            },
            "continuity": {"mode": "transcript_only"}
        }))
        .expect("replacement open result")
    }

    #[cfg(feature = "openai-live")]
    fn replacement_capability_provider(
        activator: Arc<PendingReplacementActivator>,
    ) -> LiveCapabilityProvider {
        LiveCapabilityProvider {
            configured: Some(Arc::new(ConfiguredLiveCapabilityProvider {
                factory: Arc::new(AgentFactory::minimal()),
                realm: RealmId::parse("mob.example").expect("realm"),
                experimental_factory: None,
                open_authority: Arc::new(NoBinderOpenAuthority),
                answer_transport: Arc::new(UnusedAnswerTransport),
                public_observation_publisher: Arc::new(UnusedPublicObservationPublisher),
                activator: ExperimentalLiveActivatorRegistration::Composed(activator),
                live_adapter_host: None,
                phase_authority_composed: false,
            })),
        }
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn replacement_read_uses_canonical_identity_and_is_retryable_until_bind() {
        let activator = Arc::new(PendingReplacementActivator {
            reads: AtomicUsize::new(0),
            pending: meerkat::surface::ExperimentalLiveReplacementRequired::CanonicalContext {
                open: replacement_open_result("fresh-replacement-channel"),
                canonical_seed_cursor: 17,
                pending_receipt: "replacement-pending-receipt".to_string(),
            },
        });
        let provider = replacement_capability_provider(Arc::clone(&activator));
        let session_id = SessionId::new();
        let params = serde_json::json!({"identity": "identity:canonical"});
        let response = handle_live_replacement_required(
            &provider,
            &session_id,
            Some("identity:canonical".to_string()),
            &params,
            serde_json::json!("replacement"),
        )
        .await;
        let result = response.result.expect("replacement result");
        assert_eq!(result["required"], serde_json::json!(true));
        assert_eq!(
            result["replacement"]["target_identity"],
            serde_json::json!("identity:canonical"),
        );
        assert_eq!(
            result["replacement"]["channel_id"],
            serde_json::json!("fresh-replacement-channel")
        );
        assert_eq!(activator.reads.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl ExactLiveSessionOwner for StaticSessionOwner {
        async fn owns_session(&self, _canonical_session_id: &SessionId) -> bool {
            self.0
        }

        async fn validate_live_durable_source_availability(
            &self,
            _canonical_session_id: &SessionId,
        ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError>
        {
            if self.0 {
                Ok(())
            } else {
                Err(
                    meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable,
                )
            }
        }

        async fn observe_live_durable_source_readiness(
            &self,
            canonical_session_id: &SessionId,
        ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError>
        {
            self.validate_live_durable_source_availability(canonical_session_id)
                .await
        }
    }

    #[cfg(feature = "openai-live")]
    struct UnavailableDurableSourceOwner;

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl ExactLiveSessionOwner for UnavailableDurableSourceOwner {
        async fn owns_session(&self, _canonical_session_id: &SessionId) -> bool {
            true
        }

        async fn validate_live_durable_source_availability(
            &self,
            _canonical_session_id: &SessionId,
        ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError>
        {
            Err(
                meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable,
            )
        }

        async fn observe_live_durable_source_readiness(
            &self,
            _canonical_session_id: &SessionId,
        ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError>
        {
            Err(
                meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable,
            )
        }
    }

    #[cfg(feature = "openai-live")]
    struct CountingCredentialPolicy {
        calls: AtomicUsize,
        allow: bool,
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl MetaExperimentalLiveBindingUsePolicy for CountingCredentialPolicy {
        async fn authorize_binding_use(
            &self,
            _canonical_session_id: &SessionId,
            selected_binding: &meerkat_core::AuthBindingRef,
        ) -> Result<
            meerkat_core::AuthBindingUseWitness,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.allow {
                return Err(
                    meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::BindingUseDenied,
                );
            }
            let principal =
                meerkat_core::PrincipalRef::new(meerkat_core::PrincipalKind::Human, "root")
                    .expect("principal");
            let target = meerkat_core::PrincipalRef::new(
                meerkat_core::PrincipalKind::PersonalAgent,
                "identity:robot",
            )
            .expect("target");
            let request = meerkat_core::AuthBindingUseRequest::new(
                principal.clone(),
                target.clone(),
                selected_binding.clone(),
            );
            let grant = meerkat_core::AuthGrant {
                principal: principal.clone(),
                scope: meerkat_core::GrantScope::AuthBinding {
                    realm_id: selected_binding.realm.clone(),
                    binding_id: selected_binding.binding.clone(),
                    profile_id: selected_binding.profile.clone(),
                },
                actions: std::collections::BTreeSet::from([
                    meerkat_core::GrantAction::UseAuthBinding,
                ]),
                acting_on_behalf_of: Some(meerkat_core::ActingOnBehalfOf::new(principal, target)),
            };
            meerkat_core::authorize_explicit_auth_binding_use(&request, &[grant])
                .into_result()
                .map_err(|_| {
                    meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::BindingUseDenied
                })
        }
    }

    #[cfg(feature = "openai-live")]
    fn test_live_machine() -> Arc<MeerkatMachine> {
        Arc::new(MeerkatMachine::ephemeral())
    }

    #[cfg(feature = "openai-live")]
    fn live_binding() -> meerkat_core::AuthBindingRef {
        meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("mob.example").expect("realm"),
            binding: meerkat_core::BindingId::parse("chatgpt").expect("binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }
    }

    #[cfg(feature = "openai-live")]
    fn access_view(subject: Option<&str>) -> AccessView {
        crate::access::AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec!["root".to_string()],
            ..Default::default()
        })
        .expect("access config")
        .view_for_subject(subject)
    }

    #[cfg(feature = "openai-live")]
    fn expect_live_binding_authority_error(
        result: Result<
            meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthorization,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
        >,
        message: &str,
    ) -> meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError {
        match result {
            Err(error) => error,
            Ok(_) => panic!("{message}"),
        }
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn stale_session_is_denied_before_credential_policy() {
        let credential_policy = Arc::new(CountingCredentialPolicy {
            calls: AtomicUsize::new(0),
            allow: false,
        });
        let authority = MetaExperimentalLiveSessionBindingAuthority {
            owner: Arc::new(StaticSessionOwner(false)),
            machine: test_live_machine(),
            durable_identity: "identity:robot".to_string(),
            access_view: access_view(Some("root")),
            credential_policy: credential_policy.clone(),
        };

        let error = expect_live_binding_authority_error(meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthority::authorize_binding_use(
            &authority,
            &test_session_id(),
            &live_binding(),
        )
        .await, "stale session must fail closed");
        assert!(matches!(
            error,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable
        ));
        assert_eq!(credential_policy.calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn unavailable_durable_source_is_denied_before_credential_policy() {
        let credential_policy = Arc::new(CountingCredentialPolicy {
            calls: AtomicUsize::new(0),
            allow: true,
        });
        let authority = MetaExperimentalLiveSessionBindingAuthority {
            owner: Arc::new(UnavailableDurableSourceOwner),
            machine: test_live_machine(),
            durable_identity: "identity:robot".to_string(),
            access_view: access_view(Some("root")),
            credential_policy: credential_policy.clone(),
        };

        let error = meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthority::validate_live_durable_source_availability(
            &authority,
            &test_session_id(),
        )
        .await
        .expect_err("unavailable exact durable source must fail closed");
        assert!(matches!(
            error,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable
        ));
        assert_eq!(credential_policy.calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn agent_send_denial_is_enforced_before_credential_policy() {
        let credential_policy = Arc::new(CountingCredentialPolicy {
            calls: AtomicUsize::new(0),
            allow: false,
        });
        let authority = MetaExperimentalLiveSessionBindingAuthority {
            owner: Arc::new(StaticSessionOwner(true)),
            machine: test_live_machine(),
            durable_identity: "identity:robot".to_string(),
            access_view: access_view(Some("alice")),
            credential_policy: credential_policy.clone(),
        };

        let error = expect_live_binding_authority_error(meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthority::authorize_binding_use(
            &authority,
            &test_session_id(),
            &live_binding(),
        )
        .await, "agent.send denial must fail closed");
        assert!(matches!(
            error,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::AccessDenied
        ));
        assert_eq!(credential_policy.calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn missing_principal_is_denied_before_credential_policy() {
        let credential_policy = Arc::new(CountingCredentialPolicy {
            calls: AtomicUsize::new(0),
            allow: false,
        });
        let authority = MetaExperimentalLiveSessionBindingAuthority {
            owner: Arc::new(StaticSessionOwner(true)),
            machine: test_live_machine(),
            durable_identity: "identity:robot".to_string(),
            access_view: access_view(None),
            credential_policy: credential_policy.clone(),
        };

        let error = expect_live_binding_authority_error(meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthority::authorize_binding_use(
            &authority,
            &test_session_id(),
            &live_binding(),
        )
        .await, "missing principal must fail closed");
        assert!(matches!(
            error,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::AccessDenied
        ));
        assert_eq!(credential_policy.calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn durable_source_preflight_and_exact_access_return_machine_authorization() {
        let credential_policy = Arc::new(CountingCredentialPolicy {
            calls: AtomicUsize::new(0),
            allow: true,
        });
        let authority = MetaExperimentalLiveSessionBindingAuthority {
            owner: Arc::new(StaticSessionOwner(true)),
            machine: test_live_machine(),
            durable_identity: "identity:robot".to_string(),
            access_view: access_view(Some("root")),
            credential_policy: credential_policy.clone(),
        };

        meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthority::validate_live_durable_source_availability(
            &authority,
            &test_session_id(),
        )
        .await
        .expect("available exact durable source should pass before binding use");
        meerkat::experimental_gpt_live::ExperimentalLiveSessionBindingAuthority::authorize_binding_use(
            &authority,
            &test_session_id(),
            &live_binding(),
        )
        .await
        .expect("exact target and binding policy should return machine authorization");
        assert_eq!(credential_policy.calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "openai-live-test")]
    async fn stage_receipt_answer_on_machine(
        machine: Arc<MeerkatMachine>,
        token: &str,
    ) -> (
        SessionId,
        LiveChannelId,
        meerkat_runtime::meerkat_machine::LivePlaybackOwnerReadinessAuthority,
    ) {
        let session_id = SessionId::new();
        machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare exact answer runtime binding");
        let channel_id = LiveChannelId::new(format!("answer-{}", uuid::Uuid::new_v4()));
        let identity = SessionLlmIdentity {
            model: "gpt-realtime-2".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        machine
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("admit exact answer channel");
        let execution_profile =
            meerkat_runtime::live_execution::LiveExecutionProfileSelection::__test_new(
                "meta-test-function-bridge",
                meerkat_core::LiveExecutionMode::FunctionBridge,
                meerkat_core::LiveExecutionCapabilities {
                    function_bridge: true,
                    client_context: false,
                },
            )
            .expect("test execution profile");
        machine
            .resolve_live_execution_profile_admission(&session_id, &channel_id, &execution_profile)
            .await
            .expect("resolve test live execution profile");
        let stage = machine
            .stage_experimental_live_execution(&session_id, &channel_id, 0)
            .await
            .expect("stage experimental answer execution");
        let readiness = machine
            .register_live_playback_owner(&stage, "meta-test-playback-owner")
            .await
            .expect("register test playback owner");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_millis() as u64;
        machine
            .record_live_webrtc_token_issued(&session_id, &channel_id, token, now, 60_000)
            .await
            .expect("issue exact answer token");
        (session_id, channel_id, readiness)
    }

    #[cfg(feature = "openai-live-test")]
    async fn staged_receipt_answer_machine(
        token: &str,
    ) -> (
        Arc<MeerkatMachine>,
        SessionId,
        LiveChannelId,
        meerkat_runtime::meerkat_machine::LivePlaybackOwnerReadinessAuthority,
    ) {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let (session_id, channel_id, readiness) =
            stage_receipt_answer_on_machine(Arc::clone(&machine), token).await;
        (machine, session_id, channel_id, readiness)
    }

    #[cfg(feature = "openai-live-test")]
    async fn coordinated_receipt_answer_fixture(
        token: &str,
    ) -> (
        meerkat::surface::CoordinatedLiveWebrtcAnswer,
        Arc<ReceiptAnswerTransport>,
        Arc<OrderingActivator>,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        Arc<MeerkatMachine>,
        SessionId,
        LiveChannelId,
        meerkat_runtime::meerkat_machine::LivePlaybackOwnerReadinessAuthority,
    ) {
        let (machine, session_id, channel_id, readiness) =
            staged_receipt_answer_machine(token).await;
        let machine_bound = Arc::new(AtomicBool::new(false));
        let committed = Arc::new(AtomicBool::new(false));
        let rolled_back = Arc::new(AtomicBool::new(false));
        let activator = Arc::new(OrderingActivator {
            machine_bound: Arc::clone(&machine_bound),
            calls: AtomicUsize::new(0),
        });
        let binder: Arc<dyn meerkat::surface::LiveWebrtcBoundReadyBinder> =
            Arc::new(OrderingBinder {
                activator: Arc::clone(&activator),
                machine_bound,
                committed: Arc::clone(&committed),
                rolled_back: Arc::clone(&rolled_back),
            });
        let transport = Arc::new(ReceiptAnswerTransport {
            accepted: AtomicUsize::new(0),
            rejected: AtomicUsize::new(0),
        });
        let answer = meerkat::surface::coordinate_live_webrtc_answer(
            Arc::clone(&machine),
            Arc::clone(&transport) as Arc<dyn meerkat_live::LiveWebrtcAnswerTransport>,
            Some(binder),
            channel_id.clone(),
            token.to_string(),
            "receipt-offer".to_string(),
        )
        .await
        .expect("coordinate receipt-bearing answer");
        (
            answer,
            transport,
            activator,
            committed,
            rolled_back,
            machine,
            session_id,
            channel_id,
            readiness,
        )
    }

    #[cfg(feature = "openai-live-test")]
    #[tokio::test]
    async fn receipt_answer_activates_after_atomic_bind_and_commits_after_delivery() {
        let (
            answer,
            transport,
            activator,
            committed,
            rolled_back,
            _machine,
            _session_id,
            _channel_id,
            _readiness,
        ) = coordinated_receipt_answer_fixture("receipt-delivered-token").await;
        assert_eq!(answer.answer_sdp, "receipt-answer");
        assert_eq!(
            activator.calls.load(Ordering::SeqCst),
            0,
            "prepared activation must not run before outer answer publication"
        );
        assert_eq!(transport.accepted.load(Ordering::SeqCst), 0);
        answer
            .delivery_custody
            .delivered()
            .await
            .expect("delivery settlement");
        assert_eq!(activator.calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.accepted.load(Ordering::SeqCst), 1);
        assert_eq!(transport.rejected.load(Ordering::SeqCst), 0);
        assert!(committed.load(Ordering::SeqCst));
        assert!(!rolled_back.load(Ordering::SeqCst));
    }

    #[cfg(feature = "openai-live-test")]
    #[tokio::test]
    async fn failed_outer_publication_rejects_transport_without_binding_pending_custody() {
        let (
            answer,
            transport,
            activator,
            committed,
            rolled_back,
            _machine,
            _session_id,
            _channel_id,
            _readiness,
        ) = coordinated_receipt_answer_fixture("receipt-rejected-token").await;
        let mut response = crate::rpc::SerializedRpcResponseDelivery::with_delivery_for_test(
            serde_json::json!({ "answer_sdp": answer.answer_sdp }).to_string(),
            answer.delivery_custody,
        );
        assert_eq!(transport.rejected.load(Ordering::SeqCst), 0);
        response
            .settle_delivery(false)
            .await
            .expect("failed writer settles rejection");
        assert_eq!(transport.accepted.load(Ordering::SeqCst), 0);
        assert_eq!(transport.rejected.load(Ordering::SeqCst), 1);
        assert_eq!(
            activator.calls.load(Ordering::SeqCst),
            0,
            "failed publication must discard pending readiness before binding"
        );
        assert!(!committed.load(Ordering::SeqCst));
        assert!(
            !rolled_back.load(Ordering::SeqCst),
            "no bound custody exists to roll back before successful publication"
        );
    }

    #[cfg(feature = "openai-live-test")]
    #[tokio::test]
    async fn playback_owner_loss_rpc_revokes_active_receipt_authority() {
        let persistence = meerkat::PersistenceBundle::new(
            Arc::new(meerkat::MemoryStore::new()),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("owner-loss fixture persistence");
        let temp = tempfile::tempdir().expect("owner-loss fixture state");
        let factory = AgentFactory::new(temp.path()).builtins(false);
        let mut builder = meerkat::FactoryAgentBuilder::new(factory.clone(), Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let (service, machine) =
            meerkat::surface::build_runtime_backed_service(builder, 4, persistence)
                .expect("compose the runtime-backed service");
        let service = Arc::new(service);
        let ctx = Arc::new(attach_live(
            Arc::clone(&service),
            Arc::clone(&machine),
            &factory,
            Config::default(),
            "ws://127.0.0.1/owner-loss".to_string(),
            None,
        ));
        let handler = live_rpc_handler(ctx, service, Arc::clone(&machine));
        let token = "receipt-owner-loss-token";
        let (session_id, channel_id, readiness) =
            stage_receipt_answer_on_machine(Arc::clone(&machine), token).await;
        let machine_bound = Arc::new(AtomicBool::new(false));
        let committed = Arc::new(AtomicBool::new(false));
        let rolled_back = Arc::new(AtomicBool::new(false));
        let activator = Arc::new(OrderingActivator {
            machine_bound: Arc::clone(&machine_bound),
            calls: AtomicUsize::new(0),
        });
        let binder: Arc<dyn meerkat::surface::LiveWebrtcBoundReadyBinder> =
            Arc::new(OrderingBinder {
                activator,
                machine_bound,
                committed,
                rolled_back,
            });
        let transport = Arc::new(ReceiptAnswerTransport {
            accepted: AtomicUsize::new(0),
            rejected: AtomicUsize::new(0),
        });
        let answer = meerkat::surface::coordinate_live_webrtc_answer(
            Arc::clone(&machine),
            transport as Arc<dyn meerkat_live::LiveWebrtcAnswerTransport>,
            Some(binder),
            channel_id.clone(),
            token.to_string(),
            "receipt-offer".to_string(),
        )
        .await
        .expect("coordinate active owner fixture");
        answer
            .delivery_custody
            .delivered()
            .await
            .expect("activate exact test channel");
        let active = machine
            .validate_live_channel_custody_by_pending_receipt(
                &session_id,
                &channel_id,
                readiness.pending_receipt(),
            )
            .await
            .expect("active custody before owner loss");
        let activation_receipt = match active.state() {
            meerkat_runtime::meerkat_machine::LiveChannelCustodyState::Active(receipt) => {
                receipt.activation_receipt().to_string()
            }
            phase => panic!("expected active custody, got {phase:?}"),
        };
        let response = handler
            .dispatch(
                LiveSurfaceAuthority::host_trusted_stdio(),
                Some(session_id.clone()),
                Some("identity:robot".to_string()),
                "meta/live/playback_owner/revoke".to_string(),
                serde_json::json!({
                    "identity": "identity:robot",
                    "channel_id": channel_id.as_str(),
                    "pending_receipt": readiness.pending_receipt(),
                    "readiness_receipt": readiness.readiness_id(),
                    "activation_receipt": activation_receipt,
                }),
                serde_json::json!("owner-loss"),
            )
            .await;
        assert!(
            response.error.is_none(),
            "owner-loss RPC failed: {response:?}"
        );
        assert_eq!(
            response.result,
            Some(serde_json::json!({"phase": "revoked"}))
        );
        assert!(
            machine
                .validate_live_channel_activation_receipt(
                    &session_id,
                    &channel_id,
                    &activation_receipt,
                )
                .await
                .is_err(),
            "owner loss must invalidate active provider authority"
        );
        let revoked = machine
            .validate_live_channel_custody_by_pending_receipt(
                &session_id,
                &channel_id,
                readiness.pending_receipt(),
            )
            .await
            .expect("revoked tombstone remains queryable");
        assert!(matches!(
            revoked.state(),
            meerkat_runtime::meerkat_machine::LiveChannelCustodyState::Revoked
        ));
        let status = handler
            .dispatch(
                LiveSurfaceAuthority::host_trusted_stdio(),
                Some(session_id),
                Some("identity:robot".to_string()),
                "meta/live/status".to_string(),
                serde_json::json!({
                    "identity": "identity:robot",
                    "channel_id": channel_id.as_str(),
                    "pending_receipt": readiness.pending_receipt(),
                }),
                serde_json::json!("status-after-owner-loss"),
            )
            .await;
        assert_eq!(status.result, Some(serde_json::json!({"phase": "revoked"})));
    }

    #[cfg(feature = "openai-live")]
    struct CountingOpenPublicationCleanup {
        calls: Arc<AtomicUsize>,
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl LiveOpenPublicationCleanup for CountingOpenPublicationCleanup {
        async fn cleanup(&self) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[cfg(feature = "openai-live")]
    fn open_publication_response(
        cleanup_calls: Arc<AtomicUsize>,
    ) -> crate::rpc::SerializedRpcResponseDelivery {
        let cleanup: Arc<dyn LiveOpenPublicationCleanup> =
            Arc::new(CountingOpenPublicationCleanup {
                calls: cleanup_calls,
            });
        crate::rpc::SerializedRpcResponseDelivery::with_open_delivery_for_test(
            serde_json::json!({ "channel_id": "fresh-open" }).to_string(),
            LiveOpenResponseDeliveryCustody::new(cleanup),
        )
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn strict_open_delivery_commits_publication_without_cleanup() {
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let mut response = open_publication_response(Arc::clone(&cleanup_calls));
        response
            .settle_delivery(true)
            .await
            .expect("successful writer settles open publication");
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn strict_open_rejected_publication_runs_exact_cleanup_once() {
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let mut response = open_publication_response(Arc::clone(&cleanup_calls));
        response
            .settle_delivery(false)
            .await
            .expect("failed writer settles open cleanup");
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
        response
            .settle_delivery(false)
            .await
            .expect("cleanup settlement is one-use");
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn dropped_strict_open_publication_schedules_cleanup() {
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        drop(open_publication_response(Arc::clone(&cleanup_calls)));
        tokio::task::yield_now().await;
        for _ in 0..20 {
            if cleanup_calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "openai-live")]
    fn test_session_id() -> SessionId {
        SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap()
    }

    /// An Anthropic-profile text identity carrying a realm-scoped auth
    /// binding, the downstream app's shape the cross-provider live open starts from.
    fn anthropic_identity_with_binding() -> meerkat_core::SessionLlmIdentity {
        meerkat_core::SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("mob.example").expect("realm id"),
                binding: meerkat_core::BindingId::parse("anthropic-main").expect("binding id"),
                profile: None,
                origin: meerkat_core::BindingOrigin::Configured,
            }),
        }
    }

    /// Downstream app cross-provider regression: a differing `provider` re-pairs
    /// the channel identity as a (provider, model) pair AND clears the
    /// inherited provider-specific auth binding, so the selected provider's
    /// configured default credential resolution applies for this open.
    #[test]
    fn provider_selection_repairs_identity_and_clears_auth_binding() {
        let mut identity = anthropic_identity_with_binding();
        apply_live_open_identity_selection(
            &mut identity,
            Some(meerkat_core::Provider::OpenAI),
            Some("gpt-realtime-2".to_string()),
        );
        assert_eq!(identity.provider, meerkat_core::Provider::OpenAI);
        assert_eq!(identity.model, "gpt-realtime-2");
        assert_eq!(
            identity.auth_binding, None,
            "the Anthropic binding must not ride into an OpenAI realtime open"
        );
    }

    /// A `provider` matching the inherited one is a no-op beyond the model
    /// swap: the member's binding stays valid for its own provider.
    #[test]
    fn matching_provider_selection_keeps_the_inherited_auth_binding() {
        let mut identity = anthropic_identity_with_binding();
        let inherited_binding = identity.auth_binding.clone();
        apply_live_open_identity_selection(
            &mut identity,
            Some(meerkat_core::Provider::Anthropic),
            Some("claude-opus-5".to_string()),
        );
        assert_eq!(identity.provider, meerkat_core::Provider::Anthropic);
        assert_eq!(identity.model, "claude-opus-5");
        assert_eq!(identity.auth_binding, inherited_binding);
    }

    /// Absent `provider` is the pre-existing v1 surface, byte-identical:
    /// `model` alone swaps the model, absent both leaves the identity
    /// untouched - provider and auth binding are never mutated.
    #[test]
    fn absent_provider_selection_preserves_the_legacy_model_override() {
        let mut identity = anthropic_identity_with_binding();
        let inherited_binding = identity.auth_binding.clone();

        apply_live_open_identity_selection(&mut identity, None, Some("gpt-realtime-2".to_string()));
        assert_eq!(identity.provider, meerkat_core::Provider::Anthropic);
        assert_eq!(identity.model, "gpt-realtime-2");
        assert_eq!(identity.auth_binding, inherited_binding);

        let untouched = anthropic_identity_with_binding();
        let mut identity = anthropic_identity_with_binding();
        apply_live_open_identity_selection(&mut identity, None, None);
        assert_eq!(identity, untouched);
    }

    /// `EnvRealtimeConfigSource` serves exactly the config it was given —
    /// per-open resolution then rides the session identity's auth binding
    /// against that config, matching text-model behaviour.
    #[tokio::test]
    async fn env_realtime_config_source_returns_given_config() {
        let mut config = Config::default();
        config.realm.insert(
            "live-wiring-test".to_string(),
            meerkat_core::RealmConfigSection::default(),
        );

        let source = EnvRealtimeConfigSource::new(config);
        let served = RealtimeCurrentConfigSource::current_config(&source)
            .await
            .expect("static config source");
        assert!(served.realm.contains_key("live-wiring-test"));

        // Stable across opens: the source never consults the environment or
        // mutates between calls.
        let served_again = RealtimeCurrentConfigSource::current_config(&source)
            .await
            .expect("static config source");
        assert!(served_again.realm.contains_key("live-wiring-test"));

        #[cfg(feature = "openai-live")]
        {
            let experimental = meerkat::experimental_gpt_live::ExperimentalLiveCurrentConfigSource::current_config(&source)
                .await
                .expect("experimental static config source");
            assert!(experimental.realm.contains_key("live-wiring-test"));
        }
    }
}

#[cfg(all(test, feature = "openai-live"))]
mod live_close_error_code_tests {
    use super::{INTERNAL_ERROR_CODE, INVALID_PARAMS_CODE, live_close_error_code};
    use meerkat::surface::ExperimentalLiveChannelCloseError as CloseError;

    #[test]
    fn caller_side_close_rejections_are_invalid_params() {
        assert_eq!(
            live_close_error_code(&CloseError::BindingMismatch),
            INVALID_PARAMS_CODE
        );
        assert_eq!(
            live_close_error_code(&CloseError::LifecycleAuthority(
                "receipt mismatch".to_string()
            )),
            INVALID_PARAMS_CODE
        );
    }

    #[test]
    fn gateway_side_close_failures_are_retryable_internal_errors() {
        // A dead remote that the owner has not yet retired surfaces as a
        // physical authority failure; the console retries this code on its
        // bounded schedule.
        assert_eq!(
            live_close_error_code(&CloseError::PhysicalAuthority(
                meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::ChannelBindingFailed
            )),
            INTERNAL_ERROR_CODE
        );
    }
}
