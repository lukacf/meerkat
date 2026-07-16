//! Surface-agnostic error types for the session runtime.
//!
//! Surfaces map these onto their wire shape (`RpcError`, axum
//! response, `JsError`, …). Nothing here references a wire type so
//! every consumer can adapt without pulling in `meerkat-rpc`.

use meerkat_core::SessionId;
use meerkat_core::SurfaceSessionRecoveryError;
use meerkat_core::service::SessionError;

#[cfg(all(feature = "live", not(target_arch = "wasm32")))]
use super::live_orchestration::RealtimeSessionOpenProjectionError;

/// Pre-flight rejection reasons for `live/open`-style channel opens.
///
/// Mirrors the previous `meerkat_rpc::session_runtime::LiveOpenPrecheckError`
/// without the RPC-coded variants. Surfaces translate via their own
/// error map (e.g. `RpcError::INVALID_PARAMS` for `ModelNotRealtime`,
/// `RpcError::INTERNAL_ERROR` for `ProviderHasNoLiveAdapter`).
///
/// Variants:
/// * `SessionLookup` — the session id did not resolve. The wrapped
///   [`SessionError`] carries the canonical lookup failure (`NotFound`,
///   `Busy`, etc.); surfaces should not collapse it before mapping.
/// * `ModelNotRealtime` — the resolved LLM identity is not flagged
///   `realtime` in the model-capability catalog. B19 gate.
/// * `ProviderHasNoLiveAdapter` — provider is recognized but no
///   realtime factory is wired in this build (Wave 1 ships only
///   OpenAI). B18 gate. Distinguished from `ModelNotRealtime` so the
///   operator can tell "model unsupported" from "provider not
///   yet wired".
#[derive(Debug, thiserror::Error)]
pub enum LiveOpenPrecheckError {
    /// The session id did not resolve.
    #[error("failed to resolve live session {session_id}: {source}")]
    SessionLookup {
        /// The session id that failed to resolve.
        session_id: SessionId,
        /// Underlying lookup failure.
        #[source]
        source: SessionError,
    },
    /// The resolved model is not realtime-capable.
    #[error("model {model} (provider {provider}) does not support realtime")]
    ModelNotRealtime {
        /// Resolved model id.
        model: String,
        /// Resolved provider id (`"openai"`, `"anthropic"`, …).
        provider: &'static str,
    },
    /// The provider is recognized but has no live adapter wired.
    #[error("provider {provider} has no live adapter wired in this build")]
    ProviderHasNoLiveAdapter {
        /// Resolved provider id.
        provider: &'static str,
    },
}

/// Surface-agnostic recovery error for persisted-session reconstruction.
///
/// Surfaces map this onto their wire format. The `Recovery` variant
/// carries the canonical [`SurfaceSessionRecoveryError`] from
/// `meerkat-core`; `BindingPreparation` and `Session` capture the two
/// other failure modes (runtime-binding prep failure and underlying
/// service errors) the recovery flow can hit.
///
/// Variants:
/// * `Recovery` — `build_recovered_session` rejected the override or
///   the persisted snapshot is inconsistent.
/// * `BindingPreparation` — `MeerkatMachine::prepare_bindings` (or
///   `prepare_local_session_bindings`) failed; the wrapped string is the
///   underlying error rendered with `Display`.
/// * `Session` — the underlying service surfaced a [`SessionError`].
#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    /// `build_recovered_session` rejected the recovery context.
    #[error(transparent)]
    Recovery(#[from] SurfaceSessionRecoveryError),
    /// Runtime binding preparation failed for the recovered session.
    #[error("failed to prepare runtime bindings for session {session_id}: {message}")]
    BindingPreparation {
        /// Session whose bindings could not be prepared.
        session_id: SessionId,
        /// Rendered binding error.
        message: String,
    },
    /// Underlying service surfaced a [`SessionError`].
    #[error(transparent)]
    Session(#[from] SessionError),
}

impl RecoveryError {
    /// Stable error code slug for surface→wire mapping.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Recovery(SurfaceSessionRecoveryError::InvalidOverride(_)) => "INVALID_OVERRIDE",
            Self::Recovery(SurfaceSessionRecoveryError::MissingSessionMetadata(_)) => {
                "MISSING_SESSION_METADATA"
            }
            Self::Recovery(SurfaceSessionRecoveryError::MissingSessionBuildState(_)) => {
                "MISSING_SESSION_BUILD_STATE"
            }
            Self::BindingPreparation { .. } => "BINDING_PREPARATION",
            Self::Session(_) => "SESSION",
        }
    }
}

impl LiveOpenPrecheckError {
    /// Stable error code slug for surface→wire mapping. Surfaces use
    /// this to pick an `RpcError` code, an HTTP status, or an SDK
    /// error class without re-introducing match arms over private
    /// variant fields.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SessionLookup { .. } => "SESSION_LOOKUP",
            Self::ModelNotRealtime { .. } => "MODEL_NOT_REALTIME",
            Self::ProviderHasNoLiveAdapter { .. } => "PROVIDER_HAS_NO_LIVE_ADAPTER",
        }
    }
}

/// The frozen `live/open` keep-alive ingress message (ADJ-P6B-8): ONE owner
/// for the wire-visible string; the RPC surface projects it verbatim as
/// `INVALID_PARAMS`.
#[cfg(all(feature = "live", not(target_arch = "wasm32")))]
pub const LIVE_INGRESS_INVALID_SESSION_MESSAGE: &str =
    "keep_alive requires a session created with comms_name";

/// Typed failure of the S10 peer-ingress reconciliation step of the shared
/// live open pipeline (DEC-P6B-L5 / ADJ-P6B-8).
///
/// `InvalidSession` carries NO free-form string: the fixed text lives on
/// the variant ([`LIVE_INGRESS_INVALID_SESSION_MESSAGE`]) so the frozen
/// wire message has a single owner; surface hooks return the variant, they
/// never re-author the message.
#[cfg(all(feature = "live", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveIngressError {
    /// The session cannot receive peer ingress as configured
    /// (`keep_alive` without `comms_name`). Fixed text
    /// ([`LIVE_INGRESS_INVALID_SESSION_MESSAGE`]); `INVALID_PARAMS` class
    /// on the RPC surface.
    InvalidSession,
    /// Reconciliation infrastructure fault (executor spin-up, drain
    /// context update). `INTERNAL_ERROR` class on the RPC surface.
    Internal(String),
}

#[cfg(all(feature = "live", not(target_arch = "wasm32")))]
impl std::fmt::Display for LiveIngressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // ONE owner for the frozen wire string (ADJ-P6B-8).
            Self::InvalidSession => f.write_str(LIVE_INGRESS_INVALID_SESSION_MESSAGE),
            Self::Internal(message) => f.write_str(message),
        }
    }
}

#[cfg(all(feature = "live", not(target_arch = "wasm32")))]
impl std::error::Error for LiveIngressError {}

/// Typed failure vocabulary of the extracted `live/open` pipeline
/// (DEC-P6B-L3). One variant per S1-S12 failure arm; every surface owns an
/// exhaustive projection (RPC → frozen `(code, message)` pairs;
/// `ServiceMemberLiveHost` → `MemberLiveError`) with NO wildcard arm, so a
/// new pipeline failure forces every surface to decide.
#[cfg(all(feature = "live", not(target_arch = "wasm32")))]
#[derive(Debug, thiserror::Error)]
pub enum LiveOpenError {
    /// S1 (B17): the session does not exist on this host.
    #[error("session {session_id} not found")]
    SessionNotFound { session_id: SessionId },
    /// S1 (row #98): the presence read itself faulted — surfaced typed,
    /// never collapsed into "not found".
    #[error("live session presence read failed: {0}")]
    SessionStateFault(#[source] SessionError),
    /// S2 (#302): no realtime session factory is wired into this build.
    #[error("live open requires a realtime session factory; none is wired in this build")]
    RealtimeFactoryMissing,
    /// S4: building the realtime open projection failed.
    #[error("failed to build session config: {0}")]
    OpenConfig(#[source] RealtimeSessionOpenProjectionError),
    /// S5: the generated open-admission authority faulted.
    #[error("live open authority rejected admission: {0}")]
    AdmissionAuthority(#[source] meerkat_runtime::RuntimeDriverError),
    /// S5: the session already has an active live channel.
    #[error("session {session_id} already has an active live channel")]
    AdmissionRejectedAlreadyBound { session_id: SessionId },
    /// S5: the freshly minted candidate channel id collided.
    #[error("generated duplicate live channel id {channel_id}")]
    AdmissionRejectedChannelCollision { channel_id: String },
    /// S5: the session lifecycle is already Retired/Stopped.
    #[error("session lifecycle is closed to live channel admission")]
    AdmissionRejectedLifecycleClosed,
    /// S5: admission rejected without a generated reason.
    #[error("live open authority rejected admission without a reason")]
    AdmissionRejectedNoReason,
    /// S6: admission accepted but the generated host handoff was absent.
    #[error("live open admission was accepted without a generated host handoff")]
    MissingHostHandoff,
    /// S6: the host transport cache disagreed with generated admission.
    #[error(
        "live host transport cache still has active channel for session {session_id} after generated admission"
    )]
    HostOpenSessionAlreadyBound { session_id: SessionId },
    /// S6: the host channel open failed.
    #[error("{0}")]
    HostOpen(#[source] meerkat_live::LiveAdapterHostError),
    /// S7 (B19): the realtime precheck rejected the open.
    #[error(transparent)]
    Precheck(LiveOpenPrecheckError),
    /// S8 (B18): the wired factory does not support the resolved provider.
    #[error("provider {provider} has no live adapter wired in this build")]
    ProviderUnsupportedByFactory { provider: &'static str },
    /// S9: the provider adapter open failed.
    #[error("failed to open provider session: {0}")]
    AdapterOpen(#[source] meerkat_llm_core::LlmError),
    /// S9: attaching the opened adapter to the host failed.
    #[error("failed to attach adapter: {0}")]
    AdapterAttach(#[source] meerkat_live::LiveAdapterHostError),
    /// S10: peer-ingress reconciliation failed.
    #[error(transparent)]
    Ingress(LiveIngressError),
    /// S11: no transport requested and none composed.
    #[error("live/open reached handler without configured live transport")]
    NoTransportConfigured,
    /// S11: websocket requested but no WS state/base URL composed.
    #[error("live transport websocket is not configured")]
    WebsocketNotConfigured,
    /// S11: the machine token authority rejected the WS token issue.
    #[error("live WebSocket token authority rejected issue: {0}")]
    TokenMint(String),
    /// S11 (#176): the factory advertised no resolvable audio policy.
    #[error(
        "live websocket transport requires a resolved audio policy; no realtime factory audio format was available"
    )]
    AudioPolicyMissing,
    /// S11 (#176): the resolved audio policy maps to no WS binary format.
    #[error(
        "resolved live audio policy (input {input_sample_rate_hz}Hz/{input_channels}ch) has no websocket binary format the transport can negotiate"
    )]
    AudioFormatUnmappable {
        input_sample_rate_hz: u32,
        input_channels: u16,
    },
    /// S11: webrtc requested but not configured on this surface.
    #[error("live transport webrtc is not configured")]
    WebrtcNotConfigured,
    /// S11: webrtc requested but not compiled into this build.
    #[error("live transport webrtc is not compiled into this build")]
    WebrtcNotCompiled,
    /// S11 (cfg live-webrtc): wall-clock read failed while minting.
    #[error("{0}")]
    WebrtcClock(String),
    /// S11 (cfg live-webrtc): the machine webrtc token authority rejected.
    #[error("live WebRTC token authority rejected issue: {0}")]
    WebrtcTokenMint(String),
    /// S11: an unrecognized transport variant (future `LiveOpenTransport`
    /// additions fail loud, never silently degrade).
    #[error("unsupported live transport")]
    UnsupportedTransport,
}

/// Typed failure vocabulary of the extracted live channel verbs
/// (close/status/control/send_input — DEC-P6B-L3/L6). Variants carry the
/// already-resolved generated rejection authorities so surface projections
/// reproduce today's exact wire responses without re-reaching the machine.
#[cfg(all(feature = "live", not(target_arch = "wasm32")))]
#[derive(Debug, thiserror::Error)]
pub enum LiveChannelVerbError {
    /// Channel not bound to any session; recorded via the generated
    /// unbound COMMAND rejection authority (send_input/commit/interrupt/
    /// truncate lane).
    #[error("live channel {channel_id} is not bound to a session")]
    UnboundCommand {
        channel_id: String,
        authority: meerkat_runtime::meerkat_machine::LiveCommandRejectionAuthority,
        expected: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
    },
    /// Channel not bound to any session; recorded via the generated
    /// unbound CHANNEL-REQUEST rejection authority (close/status/refresh
    /// lane). `detail` mirrors the handler-side host-error rendering.
    #[error("live channel {channel_id} is not bound to a session")]
    UnboundRequest {
        channel_id: String,
        authority: meerkat_runtime::meerkat_machine::LiveChannelRequestRejectionAuthority,
        expected: meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind,
        detail: Option<String>,
    },
    /// DEC-P6B-L6: the caller pinned an expected owning session and the
    /// machine-resolved owner differs. Fails BEFORE any side effect; a
    /// channel of another member is unaddressable through this lane.
    #[error("live channel {channel_id} is not bound to the addressed session")]
    SessionPinMismatch { channel_id: String },
    /// A generated rejection authority itself faulted; `message` is the
    /// frozen handler string (single owner: the pipeline formats it).
    #[error("{message}")]
    RejectionAuthorityFailed { message: String },
    /// Host error with a resolved generated COMMAND rejection authority.
    #[error("live command rejected for channel {channel_id}: {detail}")]
    CommandRejected {
        channel_id: String,
        authority: meerkat_runtime::meerkat_machine::LiveCommandRejectionAuthority,
        expected: meerkat_runtime::meerkat_machine::dsl::LiveCommandPublicKind,
        detail: String,
        /// Original typed host rejection. Surfaces that expose structured
        /// adapter error data must not reparse `detail` to recover it.
        host_error: Box<meerkat_live::LiveAdapterHostError>,
    },
    /// Host error with a resolved generated CHANNEL-REQUEST rejection
    /// authority.
    #[error("live channel request rejected for channel {channel_id}: {detail:?}")]
    RequestRejected {
        channel_id: String,
        authority: meerkat_runtime::meerkat_machine::LiveChannelRequestRejectionAuthority,
        expected: meerkat_runtime::meerkat_machine::dsl::LiveChannelRequestPublicKind,
        detail: Option<String>,
    },
    /// A generated result authority rejected the observation; `message` is
    /// the frozen handler string.
    #[error("{message}")]
    ResultAuthority { message: String },
    /// The generated close result omitted the host commit handoff.
    #[error("live close authority omitted host commit handoff")]
    CommitOmitted,
    /// The host close commit failed after generated authority.
    #[error("live close host commit failed after generated authority: {message}")]
    HostCommit { message: String },
    /// A generated authority emitted a malformed/mismatched result;
    /// `message` is the frozen handler string.
    #[error("{message}")]
    ResultProjection { message: String },
    /// Refresh could not rebuild the session open config.
    #[error("failed to build session config: {0}")]
    RefreshConfig(#[source] SessionError),
}
