//! Surface-agnostic error types for the session runtime.
//!
//! Surfaces map these onto their wire shape (`RpcError`, axum
//! response, `JsError`, …). Nothing here references a wire type so
//! every consumer can adapt without pulling in `meerkat-rpc`.

use meerkat_core::SessionId;
use meerkat_core::service::SessionError;

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
