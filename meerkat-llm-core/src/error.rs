//! LLM client errors
//!
//! Categorized by whether they're retryable.

use meerkat_core::error::{LlmFailureReason, LlmProviderError, LlmProviderErrorKind};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

/// Errors from LLM providers
///
/// Categorized by whether they're retryable.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
pub enum LlmError {
    // === Retryable Errors ===
    #[error("Rate limited{}", match .retry_after_ms {
        Some(ms) => format!(", retry after {ms}ms"),
        None => String::new(),
    })]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("Server overloaded (503)")]
    ServerOverloaded,

    #[error("Network timeout after {duration_ms}ms")]
    NetworkTimeout { duration_ms: u64 },

    #[error("Connection reset")]
    ConnectionReset,

    #[error("Server error: {status} - {message}")]
    ServerError { status: u16, message: String },

    // === Non-Retryable Errors ===
    #[error("Invalid request: {message}")]
    InvalidRequest { message: String },

    /// The serialized request body exceeds the provider request-size cap.
    /// This remains distinct from generic invalid input so callers can recover
    /// without rediscovering the class by matching provider prose.
    #[error("Request too large: {message}")]
    RequestTooLarge {
        message: String,
        encoded_bytes: Option<u64>,
        max_bytes: Option<u64>,
    },

    /// Caller sent a request shape this provider does not support
    /// (e.g. image / video-frame input on a realtime audio channel,
    /// malformed audio chunk variant). Scoped, non-terminal: the
    /// channel survives so the client can retry with a supported
    /// shape. Distinct from `InvalidRequest` so consumers can branch
    /// on it without matching error message strings.
    #[error("Invalid input shape: {message}")]
    InvalidInputShape { message: String },

    /// Provider-config rejection (model swap, audio rate mismatch,
    /// refresh-time invariant, voice/format incompatibility). Terminal
    /// on this channel: the caller must close + reopen rather than
    /// retry the same request. Distinct from `InvalidRequest` so the
    /// classification is structural, not a string match.
    #[error("Invalid provider config: {message}")]
    InvalidConfig { message: String },

    #[error("Authentication failed: {message}")]
    AuthenticationFailed { message: String },

    /// A dynamic authorizer refreshed successfully but changed the concrete
    /// provider route before any response bytes were consumed. The caller must
    /// retry above request projection so provider-specific lowering is rebuilt.
    #[error("Authorization route changed: {message}")]
    AuthorizationRouteChanged { message: String },

    #[error("Content filtered: {reason}")]
    ContentFiltered { reason: String },

    #[error("Context length exceeded: {requested} > {max}")]
    ContextLengthExceeded { max: usize, requested: usize },

    #[error("Model not found: {model}")]
    ModelNotFound { model: String },

    #[error("Invalid API key")]
    InvalidApiKey,

    /// The credential is valid but the account behind it has no remaining
    /// quota, prepaid credit, or spend allowance: OpenAI `insufficient_quota`,
    /// `credit_balance_exhausted` and the spend-limit codes, Anthropic's
    /// monthly spend cap (`enforced_spend_limit_reached`), self-set spend
    /// limit and HTTP 402 `billing_error`, Gemini `quota_exceeded` or a daily
    /// `QuotaFailure`. Providers front most of these with HTTP 429, but no
    /// retry inside a turn can clear them: an operator must add credit, raise
    /// the limit, or wait for the billing window to reset. Non-retryable so a
    /// dead key fails in one round trip instead of after the bounded
    /// rate-limit retry window.
    #[error("Provider quota exhausted: {message}")]
    QuotaExhausted { message: String },

    // === Unknown ===
    /// The provider returned a failure class this client does not yet know.
    /// This is deliberately retryable: absence of a known terminal class is
    /// not evidence that the turn should terminalize.
    /// [`RetryPolicy`](meerkat_core::retry::RetryPolicy) still bounds attempts
    /// and backoff.
    #[error("Unknown error: {message}")]
    Unknown { message: String },

    // === Streaming Errors ===
    #[error("Stream parsing error: {message}")]
    StreamParseError { message: String },

    #[error("Incomplete response: {message}")]
    IncompleteResponse { message: String },
}

/// Provider error codes that name exhausted quota, prepaid credit, or a spend
/// allowance rather than a transient rate limit. Each is documented by its
/// provider as failing until an operator acts or a billing window resets:
/// OpenAI `insufficient_quota` (the production 429 body that motivated this
/// class), `credit_balance_exhausted`, `organization_spend_limit_exceeded`,
/// `project_spend_limit_exceeded`, `organization_usage_limit_exceeded` and the
/// legacy `billing_hard_limit_reached`; Anthropic `enforced_spend_limit_reached`
/// (carried in `error.details.error_code` on a `rate_limit_error` 429 that has
/// no `retry-after`, documented under "Reaching your spend cap" at
/// <https://platform.claude.com/docs/en/api/rate-limits>, read 2026-09-04);
/// Gemini `quota_exceeded`, listed in the Gemini API error-code table at
/// <https://ai.google.dev/gemini-api/docs/api-errors> (read 2026-09-04) as a
/// 429 whose documented text is "You have exceeded your daily quota. Wait
/// until the quota resets or request a quota increase." The neighbouring
/// per-minute codes on that table (`rate_limit_exceeded`, `too_many_requests`)
/// are deliberately absent so they keep the retryable rate-limit class.
const QUOTA_EXHAUSTED_ERROR_CODES: &[&str] = &[
    "insufficient_quota",
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "organization_usage_limit_exceeded",
    "billing_hard_limit_reached",
    "enforced_spend_limit_reached",
    "quota_exceeded",
];

/// Anthropic reports a self-configured organization or workspace spend limit
/// as HTTP 400 `invalid_request_error` with no code; the only discriminator
/// is the message, which the platform docs state "begins" with one of these
/// two prefixes ("Setting your own spend limit" at
/// <https://platform.claude.com/docs/en/api/rate-limits>, quoted 2026-09-04).
/// This is the one prose match in the failure classification, kept because
/// the provider offers nothing structured here. Blast radius of a reworded
/// message: the 400 falls back to `InvalidRequest`, which is also
/// non-retryable, so the turn still terminalizes in one round trip and only
/// the reported class degrades. Do not extend this list without a captured
/// body or a documented quotation.
const ANTHROPIC_SELF_SET_SPEND_LIMIT_PREFIXES: &[&str] = &[
    "You have reached your specified API usage limits",
    "You have reached your specified workspace API usage limits",
];

/// Value of `details.class` on the wire projection of
/// [`LlmError::QuotaExhausted`]. The dedicated `quota_exhausted` provider
/// error kind is the primary discriminator; the class is a one-release
/// compatibility carrier for consumers that started branching on it while
/// the failure still rode the `invalid_request` kind.
pub const QUOTA_EXHAUSTED_DETAILS_CLASS: &str = "quota_exhausted";

/// Recovery hint appended to request-too-large provider rejections.
///
/// A transcript over the provider's request-size cap cannot recover by
/// retrying the identical turn. Provider-backed compaction can still succeed
/// when its text-only summary projection fits; curator-driven compaction is
/// the host-side recovery when even that call cannot fit. Built-in provider
/// adapters automatically attach their active cap to exact request preflight;
/// custom clients can supply an explicit `compaction.max_request_bytes`.
pub const REQUEST_TOO_LARGE_RECOVERY_HINT: &str = "request exceeds the provider request-size cap; \
retries cannot succeed until the transcript shrinks. Recovery: run curator-driven compaction \
(CompactionCurator produces the summary without an LLM call, so an already-over-cap session can \
still compact), reduce retained history or media, and set compaction.max_request_bytes when using \
a custom client that cannot report exact request pressure";

/// The `error` object every built-in provider returns inside an HTTP error
/// body (`{"error": {...}}`) or a streaming error event, parsed once into the
/// fields the failure classification reads. Every field is optional and
/// unknown fields are ignored, so any JSON object deserializes; a provider
/// client that already holds a parsed error object classifies it through
/// [`ProviderErrorObject::signals_quota_exhausted`] instead of walking JSON by
/// hand.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProviderErrorObject {
    /// OpenAI / Anthropic string code (`insufficient_quota`) or Google's
    /// numeric HTTP code (`429`).
    code: Option<ProviderErrorCode>,
    /// OpenAI / Anthropic error type (`rate_limit_error`; OpenAI also uses the
    /// quota code itself as the type).
    #[serde(rename = "type")]
    error_type: Option<String>,
    message: Option<String>,
    /// Google `google.rpc.Status` name (`RESOURCE_EXHAUSTED`).
    status: Option<String>,
    details: Option<ProviderErrorDetails>,
}

/// A provider error code is either a named class (OpenAI / Anthropic) or
/// Google's numeric HTTP status echo, which classifies nothing and is ignored.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProviderErrorCode {
    Named(String),
    Unnamed(serde::de::IgnoredAny),
}

/// `error.details` differs per provider: Google carries the `google.rpc.Status`
/// detail list, Anthropic nests an object carrying `error_code`. Anything else
/// is ignored.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProviderErrorDetails {
    Google(Vec<GoogleErrorDetail>),
    Anthropic(AnthropicErrorDetails),
    Other(serde::de::IgnoredAny),
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AnthropicErrorDetails {
    error_code: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GoogleErrorDetail {
    #[serde(rename = "@type")]
    type_url: String,
    violations: Vec<GoogleQuotaViolation>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GoogleQuotaViolation {
    quota_id: Option<String>,
    quota_value: Option<GoogleQuotaValue>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum GoogleQuotaValue {
    Text(String),
    Number(serde_json::Number),
}

/// An HTTP error body: the provider's error object under `error`, or the
/// error object itself at the top level.
#[derive(Deserialize)]
#[serde(untagged)]
enum ProviderErrorBody {
    Wrapped { error: ProviderErrorObject },
    Bare(ProviderErrorObject),
}

impl ProviderErrorObject {
    /// Whether this error object reports exhausted quota, prepaid credit, or a
    /// spend allowance rather than a transient rate limit. Structured codes
    /// decide first (`code`, `type`, Anthropic `details.error_code`), then the
    /// one documented Anthropic message prefix that carries no code, then
    /// Google's `RESOURCE_EXHAUSTED` `QuotaFailure` detail.
    pub fn signals_quota_exhausted(&self) -> bool {
        let named_code = match &self.code {
            Some(ProviderErrorCode::Named(code)) => Some(code.as_str()),
            Some(ProviderErrorCode::Unnamed(_)) | None => None,
        };
        if named_code
            .into_iter()
            .chain(self.error_type.as_deref())
            .any(LlmError::code_signals_quota_exhausted)
        {
            return true;
        }
        if let Some(ProviderErrorDetails::Anthropic(details)) = &self.details
            && details
                .error_code
                .as_deref()
                .is_some_and(LlmError::code_signals_quota_exhausted)
        {
            return true;
        }
        if self
            .message
            .as_deref()
            .is_some_and(LlmError::message_signals_quota_exhausted)
        {
            return true;
        }
        self.status.as_deref() == Some("RESOURCE_EXHAUSTED")
            && self.google_quota_failure_is_exhausted()
    }

    /// Gemini fronts every rate limit with `429 RESOURCE_EXHAUSTED` and the
    /// same message; the `google.rpc.QuotaFailure` detail names the violated
    /// window. A daily window (Gemini's documented RPD limit, which resets at
    /// midnight Pacific and is named `...PerDay...` in `quotaId`) or a zero
    /// entitlement (`quotaValue` `0`: the project has no quota for the model
    /// at all) cannot clear inside a turn's retry window. Per-minute
    /// violations, which arrive with a `RetryInfo` delay, stay rate limits.
    fn google_quota_failure_is_exhausted(&self) -> bool {
        let Some(ProviderErrorDetails::Google(details)) = &self.details else {
            return false;
        };
        details
            .iter()
            .filter(|detail| detail.type_url == "type.googleapis.com/google.rpc.QuotaFailure")
            .flat_map(|detail| detail.violations.iter())
            .any(|violation| {
                let daily_window = violation
                    .quota_id
                    .as_deref()
                    .is_some_and(|id| id.contains("PerDay"));
                let zero_entitlement = match &violation.quota_value {
                    Some(GoogleQuotaValue::Text(value)) => value == "0",
                    Some(GoogleQuotaValue::Number(value)) => value.as_u64() == Some(0),
                    None => false,
                };
                daily_window || zero_entitlement
            })
    }
}

impl LlmError {
    pub fn from_authorizer(error: meerkat_core::AuthError) -> Self {
        match error {
            meerkat_core::AuthError::ResolveRequired(message) => {
                Self::AuthorizationRouteChanged { message }
            }
            other => Self::AuthenticationFailed {
                message: other.to_string(),
            },
        }
    }

    /// Construct a typed request-too-large provider rejection.
    pub fn request_too_large(message: String) -> Self {
        Self::request_too_large_with_pressure(message, None, None)
    }

    /// Construct a typed request-too-large preflight verdict with exact
    /// pressure evidence, when available.
    pub fn request_too_large_with_pressure(
        message: String,
        encoded_bytes: Option<u64>,
        max_bytes: Option<u64>,
    ) -> Self {
        Self::RequestTooLarge {
            message: format!("{message} ({REQUEST_TOO_LARGE_RECOVERY_HINT})"),
            encoded_bytes,
            max_bytes,
        }
    }

    /// Whether an error body advertises the request-too-large class even when
    /// the HTTP status is not 413 (e.g. Anthropic's typed
    /// `"type":"request_too_large"` error code).
    fn body_signals_request_too_large(body: &str) -> bool {
        body.contains("request_too_large")
    }

    /// Whether a provider error code names exhausted quota, credit, or spend
    /// allowance (see `QUOTA_EXHAUSTED_ERROR_CODES`). Provider clients use
    /// this on structured stream / realtime error events, which carry the
    /// code without an HTTP body.
    pub fn code_signals_quota_exhausted(code: &str) -> bool {
        QUOTA_EXHAUSTED_ERROR_CODES.contains(&code)
    }

    fn message_signals_quota_exhausted(message: &str) -> bool {
        let message = message.trim_start();
        ANTHROPIC_SELF_SET_SPEND_LIMIT_PREFIXES
            .iter()
            .any(|prefix| message.starts_with(prefix))
    }

    /// Whether an HTTP error body reports exhausted quota. The body is parsed
    /// once into [`ProviderErrorObject`]: every built-in provider wraps its
    /// error object under a top-level `error` key, a bare object is read as
    /// the error object itself, and a non-JSON body is treated as a bare
    /// message (the streaming error paths hand over the message alone).
    fn body_signals_quota_exhausted(body: &str) -> bool {
        match serde_json::from_str::<ProviderErrorBody>(body) {
            Ok(ProviderErrorBody::Wrapped { error } | ProviderErrorBody::Bare(error)) => {
                error.signals_quota_exhausted()
            }
            Err(_) => Self::message_signals_quota_exhausted(body),
        }
    }

    /// Provider context-window rejection classification at the provider
    /// boundary. Exact structured codes are preferred; the phrase checks cover
    /// providers that expose only a message inside an `INVALID_ARGUMENT`
    /// envelope. Shared agent policy never parses this prose.
    fn body_signals_context_exceeded(body: &str) -> bool {
        let lower = body.to_ascii_lowercase();
        [
            "context_length_exceeded",
            "context_window_exceeded",
            "maximum context length",
            "prompt is too long",
            "exceeds the maximum number of tokens",
        ]
        .iter()
        .any(|signal| lower.contains(signal))
    }

    fn context_counts(body: &str) -> (usize, usize) {
        fn find_named(value: &serde_json::Value, names: &[&str]) -> Option<u64> {
            match value {
                serde_json::Value::Object(object) => names
                    .iter()
                    .find_map(|name| object.get(*name).and_then(serde_json::Value::as_u64))
                    .or_else(|| object.values().find_map(|value| find_named(value, names))),
                serde_json::Value::Array(values) => {
                    values.iter().find_map(|value| find_named(value, names))
                }
                _ => None,
            }
        }

        let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
        let max = parsed
            .as_ref()
            .and_then(|value| find_named(value, &["max", "context_window", "max_tokens"]))
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);
        let requested = parsed
            .as_ref()
            .and_then(|value| {
                find_named(
                    value,
                    &["requested", "input_tokens", "prompt_tokens", "total_tokens"],
                )
            })
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or_else(|| max.saturating_add(1));
        (max, requested)
    }

    /// Whether this error should trigger a retry
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. }
            | Self::ServerOverloaded
            | Self::NetworkTimeout { .. }
            | Self::ConnectionReset
            | Self::AuthorizationRouteChanged { .. }
            | Self::Unknown { .. } => true,
            Self::ServerError { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Get retry-after hint if available
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after_ms } => retry_after_ms.map(Duration::from_millis),
            _ => None,
        }
    }

    /// Create from HTTP status code and message
    pub fn from_http_status(status: u16, message: String, retry_after_ms: Option<u64>) -> Self {
        match status {
            401 => Self::AuthenticationFailed { message },
            // 402 is a billing failure (Anthropic `billing_error`): the key is
            // valid, the account cannot pay, and no retry clears it.
            402 => Self::QuotaExhausted { message },
            403 => Self::InvalidApiKey,
            404 => Self::ModelNotFound { model: message },
            // 413 is the request-too-large class (2026-07-29 incident:
            // an over-cap transcript fails every turn terminally); attach
            // the curator-compaction recovery hint.
            413 => Self::request_too_large(message),
            // 429 fronts both transient rate limits and exhausted quota
            // (OpenAI `insufficient_quota`, Anthropic's spend cap, Gemini's
            // daily quota). Only the former is worth the bounded retry window;
            // the latter is terminal until an operator acts.
            429 if Self::body_signals_quota_exhausted(&message) => Self::QuotaExhausted { message },
            429 => Self::RateLimited { retry_after_ms },
            503 => Self::ServerOverloaded,
            s if s >= 500 => Self::ServerError { status: s, message },
            s if s >= 400 => {
                if Self::body_signals_request_too_large(&message) {
                    Self::request_too_large(message)
                } else if Self::body_signals_context_exceeded(&message) {
                    let (max, requested) = Self::context_counts(&message);
                    Self::ContextLengthExceeded { max, requested }
                } else if Self::body_signals_quota_exhausted(&message) {
                    Self::QuotaExhausted { message }
                } else {
                    Self::InvalidRequest { message }
                }
            }
            _ => Self::Unknown { message },
        }
    }

    pub fn from_http_response(
        status: u16,
        message: String,
        headers: &reqwest::header::HeaderMap,
    ) -> Self {
        let retry_after_ms = headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(Self::parse_retry_after);
        Self::from_http_status(status, message, retry_after_ms)
    }

    pub fn parse_retry_after(value: &str) -> Option<u64> {
        if let Ok(secs) = value.trim().parse::<u64>() {
            return Some(secs * 1000);
        }
        if let Ok(secs) = value.trim().parse::<f64>()
            && secs > 0.0
        {
            return Some((secs * 1000.0) as u64);
        }
        None
    }

    pub fn failure_reason(&self) -> LlmFailureReason {
        fn as_u32(value: usize) -> u32 {
            u32::try_from(value).unwrap_or(u32::MAX)
        }

        match self {
            Self::RateLimited { retry_after_ms } => LlmFailureReason::RateLimited {
                retry_after: retry_after_ms.map(Duration::from_millis),
            },
            Self::ContextLengthExceeded { max, requested } => LlmFailureReason::ContextExceeded {
                max: as_u32(*max),
                requested: as_u32(*requested),
            },
            Self::AuthenticationFailed { .. } | Self::InvalidApiKey => LlmFailureReason::AuthError,
            Self::ModelNotFound { model } => LlmFailureReason::InvalidModel(model.clone()),
            Self::RequestTooLarge {
                message,
                encoded_bytes,
                max_bytes,
            } => LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                LlmProviderErrorKind::RequestTooLarge,
                json!({
                    "message": message,
                    "encoded_bytes": encoded_bytes,
                    "max_bytes": max_bytes,
                }),
            )),
            Self::AuthorizationRouteChanged { message } => {
                LlmFailureReason::ProviderError(LlmProviderError::retryable(
                    LlmProviderErrorKind::AuthorizationRouteChanged,
                    json!({
                        "message": message,
                    }),
                ))
            }
            // `details.class` is kept for one release: 0.8.33 projected
            // exhausted quota as `invalid_request` and consumers were told to
            // branch on the class, so it stays alongside the dedicated kind
            // until they have moved. Nothing should match the message.
            Self::QuotaExhausted { message } => {
                LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                    LlmProviderErrorKind::QuotaExhausted,
                    json!({
                        "class": QUOTA_EXHAUSTED_DETAILS_CLASS,
                        "message": message,
                    }),
                ))
            }
            Self::InvalidRequest { message }
            | Self::InvalidInputShape { message }
            | Self::InvalidConfig { message } => {
                LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                    LlmProviderErrorKind::InvalidRequest,
                    json!({
                        "message": message,
                    }),
                ))
            }
            Self::ContentFiltered { reason } => {
                LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                    LlmProviderErrorKind::ContentFiltered,
                    json!({
                        "message": reason,
                    }),
                ))
            }
            Self::ServerError { status, message } => {
                let details = json!({
                    "status": status,
                    "message": message,
                });
                if self.is_retryable() {
                    LlmFailureReason::ProviderError(LlmProviderError::retryable(
                        LlmProviderErrorKind::ServerError,
                        details,
                    ))
                } else {
                    LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                        LlmProviderErrorKind::ServerError,
                        details,
                    ))
                }
            }
            Self::ServerOverloaded => LlmFailureReason::ProviderError(LlmProviderError::retryable(
                LlmProviderErrorKind::ServerOverloaded,
                json!({
                    "message": self.to_string(),
                }),
            )),
            Self::NetworkTimeout { duration_ms } => LlmFailureReason::NetworkTimeout {
                duration_ms: *duration_ms,
            },
            Self::ConnectionReset => LlmFailureReason::ProviderError(LlmProviderError::retryable(
                LlmProviderErrorKind::ConnectionReset,
                json!({
                    "message": self.to_string(),
                }),
            )),
            Self::Unknown { message } => {
                LlmFailureReason::ProviderError(LlmProviderError::retryable(
                    LlmProviderErrorKind::Unknown,
                    json!({
                        "message": message,
                    }),
                ))
            }
            Self::StreamParseError { message } => {
                LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                    LlmProviderErrorKind::StreamParseError,
                    json!({
                        "message": message,
                    }),
                ))
            }
            Self::IncompleteResponse { message } => {
                LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                    LlmProviderErrorKind::IncompleteResponse,
                    json!({
                        "message": message,
                    }),
                ))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_retryable_errors() {
        assert!(
            LlmError::RateLimited {
                retry_after_ms: Some(1000)
            }
            .is_retryable()
        );
        assert!(LlmError::ServerOverloaded.is_retryable());
        assert!(LlmError::NetworkTimeout { duration_ms: 30000 }.is_retryable());
        assert!(LlmError::ConnectionReset.is_retryable());
        assert!(
            LlmError::AuthorizationRouteChanged {
                message: "new endpoint".to_string()
            }
            .is_retryable()
        );
        assert!(
            LlmError::ServerError {
                status: 500,
                message: "Internal error".to_string()
            }
            .is_retryable()
        );
        assert!(
            LlmError::ServerError {
                status: 502,
                message: "Bad gateway".to_string()
            }
            .is_retryable()
        );
    }

    #[test]
    fn authorization_route_change_preserves_typed_retry_class() {
        let error = LlmError::AuthorizationRouteChanged {
            message: "new endpoint".to_string(),
        };
        let LlmFailureReason::ProviderError(provider_error) = error.failure_reason() else {
            panic!("route changes must retain a provider-error carrier");
        };
        assert_eq!(
            provider_error.kind,
            LlmProviderErrorKind::AuthorizationRouteChanged
        );
        assert!(provider_error.is_retryable());
    }

    #[test]
    fn test_non_retryable_errors() {
        assert!(
            !LlmError::InvalidRequest {
                message: "Bad request".to_string()
            }
            .is_retryable()
        );
        assert!(
            !LlmError::AuthenticationFailed {
                message: "Invalid key".to_string()
            }
            .is_retryable()
        );
        assert!(!LlmError::InvalidApiKey.is_retryable());
        assert!(
            !LlmError::QuotaExhausted {
                message: "insufficient_quota".to_string()
            }
            .is_retryable()
        );
        assert!(
            !LlmError::ContentFiltered {
                reason: "Policy".to_string()
            }
            .is_retryable()
        );
        assert!(
            !LlmError::ModelNotFound {
                model: "gpt-5".to_string()
            }
            .is_retryable()
        );
    }

    #[test]
    fn unknown_provider_errors_default_to_retryable() {
        let err = LlmError::Unknown {
            message: "provider returned an unrecognized transient class".to_string(),
        };

        assert!(err.is_retryable());
        let LlmFailureReason::ProviderError(provider_error) = err.failure_reason() else {
            panic!("unknown provider failures must retain the provider-error carrier");
        };
        assert_eq!(provider_error.kind, LlmProviderErrorKind::Unknown);
        assert_eq!(
            provider_error.retryability,
            meerkat_core::error::LlmProviderErrorRetryability::Retryable
        );
    }

    #[test]
    fn test_retry_after() {
        let err = LlmError::RateLimited {
            retry_after_ms: Some(5000),
        };
        assert_eq!(err.retry_after(), Some(Duration::from_millis(5000)));

        let err = LlmError::RateLimited {
            retry_after_ms: None,
        };
        assert_eq!(err.retry_after(), None);

        let err = LlmError::ServerOverloaded;
        assert_eq!(err.retry_after(), None);
    }

    #[test]
    fn test_from_http_status() {
        assert!(matches!(
            LlmError::from_http_status(401, "".to_string(), None),
            LlmError::AuthenticationFailed { .. }
        ));
        assert!(matches!(
            LlmError::from_http_status(429, "".to_string(), None),
            LlmError::RateLimited { .. }
        ));
        assert!(matches!(
            LlmError::from_http_status(503, "".to_string(), None),
            LlmError::ServerOverloaded
        ));
        assert!(matches!(
            LlmError::from_http_status(500, "".to_string(), None),
            LlmError::ServerError { status: 500, .. }
        ));
    }

    #[test]
    fn test_error_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let errors = vec![
            LlmError::RateLimited {
                retry_after_ms: Some(1000),
            },
            LlmError::ServerOverloaded,
            LlmError::InvalidRequest {
                message: "test".to_string(),
            },
            LlmError::QuotaExhausted {
                message: "insufficient_quota".to_string(),
            },
        ];

        for err in errors {
            let json = serde_json::to_string(&err)?;
            let _: LlmError = serde_json::from_str(&json)?;
        }
        Ok(())
    }

    #[test]
    fn test_network_timeout_maps_to_typed_reason() {
        let err = LlmError::NetworkTimeout { duration_ms: 30000 };
        let reason = err.failure_reason();
        assert_eq!(
            reason,
            LlmFailureReason::NetworkTimeout { duration_ms: 30000 }
        );
    }

    #[test]
    fn provider_failure_reason_uses_typed_kind_and_retryability() {
        let reason = LlmError::ServerOverloaded.failure_reason();
        let LlmFailureReason::ProviderError(provider_error) = reason else {
            panic!("expected provider error");
        };

        assert_eq!(
            provider_error.kind,
            meerkat_core::error::LlmProviderErrorKind::ServerOverloaded
        );
        assert_eq!(
            provider_error.retryability,
            meerkat_core::error::LlmProviderErrorRetryability::Retryable
        );
        assert_eq!(
            provider_error.details["message"],
            serde_json::json!("Server overloaded (503)")
        );
        assert!(
            provider_error.details.get("kind").is_none(),
            "provider error kind must not be carried in untyped JSON"
        );
        assert!(
            provider_error.details.get("retryable").is_none(),
            "provider error retryability must not be carried in untyped JSON"
        );
    }

    // -- Retry-After parsing tests (PR #156 port) --

    #[test]
    fn test_parse_retry_after_integer_seconds() {
        assert_eq!(LlmError::parse_retry_after("120"), Some(120_000));
        assert_eq!(LlmError::parse_retry_after("1"), Some(1_000));
    }

    #[test]
    fn test_parse_retry_after_fractional_seconds() {
        assert_eq!(LlmError::parse_retry_after("0.5"), Some(500));
        assert_eq!(LlmError::parse_retry_after("1.5"), Some(1_500));
    }

    #[test]
    fn test_parse_retry_after_with_whitespace() {
        assert_eq!(LlmError::parse_retry_after("  30  "), Some(30_000));
    }

    #[test]
    fn test_parse_retry_after_invalid() {
        assert_eq!(LlmError::parse_retry_after("not-a-number"), None);
    }

    #[test]
    fn test_parse_retry_after_negative() {
        assert_eq!(LlmError::parse_retry_after("-5"), None);
    }

    #[test]
    fn test_from_http_status_429_with_retry_after() {
        let err = LlmError::from_http_status(429, "rate limited".to_string(), Some(5000));
        assert!(matches!(
            err,
            LlmError::RateLimited {
                retry_after_ms: Some(5000)
            }
        ));
    }

    #[test]
    fn test_from_http_status_429_without_retry_after() {
        let err = LlmError::from_http_status(429, "rate limited".to_string(), None);
        assert!(matches!(
            err,
            LlmError::RateLimited {
                retry_after_ms: None
            }
        ));
    }

    #[test]
    fn test_from_http_status_non_429_ignores_retry_after() {
        let err = LlmError::from_http_status(500, "server error".to_string(), Some(5000));
        assert!(matches!(err, LlmError::ServerError { status: 500, .. }));
    }

    #[test]
    fn test_http_413_maps_to_typed_request_too_large_with_recovery_hint() {
        let err = LlmError::from_http_status(413, "Request body too large".to_string(), None);
        let LlmError::RequestTooLarge { message, .. } = &err else {
            panic!("413 must map to RequestTooLarge, got {err:?}");
        };
        assert!(message.contains("Request body too large"));
        assert!(
            message.contains("CompactionCurator"),
            "request-too-large errors must carry the curator recovery hint: {message}"
        );
        assert!(!err.is_retryable());

        let reason = err.failure_reason();
        let LlmFailureReason::ProviderError(provider_error) = reason else {
            panic!("expected provider error reason");
        };
        assert_eq!(provider_error.kind, LlmProviderErrorKind::RequestTooLarge);
        assert!(
            provider_error.details["message"]
                .as_str()
                .expect("message detail")
                .contains("CompactionCurator"),
            "the hint must survive into the typed failure-reason details"
        );
    }

    #[test]
    fn test_request_too_large_body_marker_maps_hint_without_413_status() {
        // Anthropic advertises the class as a typed error code in the body;
        // the hint must attach even when the fronting status is a plain 400.
        let body = r#"{"type":"error","error":{"type":"request_too_large","message":"Request body too large"}}"#;
        let err = LlmError::from_http_status(400, body.to_string(), None);
        assert!(
            matches!(&err, LlmError::RequestTooLarge { message, .. } if message.contains("CompactionCurator")),
            "body-signaled request_too_large must carry the recovery hint: {err:?}"
        );

        // An ordinary 400 stays hint-free.
        let plain = LlmError::from_http_status(400, "bad field".to_string(), None);
        assert!(
            matches!(&plain, LlmError::InvalidRequest { message } if !message.contains("CompactionCurator")),
            "ordinary invalid requests must not claim the request-too-large recovery path"
        );
    }

    #[test]
    fn test_from_http_response_extracts_retry_after_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "30".parse().unwrap());
        let err = LlmError::from_http_response(429, "rate limited".to_string(), &headers);
        assert!(matches!(
            err,
            LlmError::RateLimited {
                retry_after_ms: Some(30_000)
            }
        ));
    }

    #[test]
    fn test_from_http_response_no_header_returns_none() {
        let headers = reqwest::header::HeaderMap::new();
        let err = LlmError::from_http_response(429, "rate limited".to_string(), &headers);
        assert!(matches!(
            err,
            LlmError::RateLimited {
                retry_after_ms: None
            }
        ));
    }

    // -- Exhausted quota is terminal, not a rate limit (A5 sub-finding) --

    /// OpenAI 429 body for a key whose account has no quota left. Retrying
    /// it three times behind the 30s rate-limit floor was the ~90s window
    /// hosts observed before the failure surfaced.
    const OPENAI_INSUFFICIENT_QUOTA_BODY: &str = r#"{"error":{"message":"You exceeded your current quota, please check your plan and billing details. For more information on this error, read the docs: https://platform.openai.com/docs/guides/error-codes/api-errors.","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}"#;

    /// Anthropic monthly spend-cap 429 (documented under "Reaching your spend
    /// cap"): the same `rate_limit_error` type as a rate limit, no
    /// retry-after, discriminated only by `details.error_code`.
    const ANTHROPIC_SPEND_CAP_BODY: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"You have reached your API usage limits: your organization has crossed its monthly API usage threshold, set based on your organization's API tier. You will regain access on 2026-09-01 at 00:00 UTC.","details":{"error_code":"enforced_spend_limit_reached"}},"request_id":"req_018EeWyXxfu5pfWkrYcMdjWG"}"#;

    /// Positive observables shared by every exhausted-quota test: the typed
    /// variant, no retry, the readable prefix, a non-retryable provider
    /// carrier, and the agent-loop retry authority admitting no plan.
    fn assert_quota_exhausted_is_terminal(
        err: &LlmError,
        provider: &'static str,
    ) -> meerkat_core::error::LlmProviderError {
        assert!(
            matches!(err, LlmError::QuotaExhausted { .. }),
            "expected QuotaExhausted, got {err:?}"
        );
        assert!(
            !err.is_retryable(),
            "exhausted quota must not enter the retry path: {err:?}"
        );
        assert_eq!(err.retry_after(), None);
        assert!(
            err.to_string().starts_with("Provider quota exhausted: "),
            "operator-facing message must name the class: {err}"
        );
        let reason = err.failure_reason();
        let LlmFailureReason::ProviderError(provider_error) = &reason else {
            panic!("exhausted quota must keep the typed provider carrier: {reason:?}");
        };
        assert_eq!(
            provider_error.kind,
            LlmProviderErrorKind::QuotaExhausted,
            "exhausted quota has its own wire kind: {provider_error:?}"
        );
        assert_eq!(
            provider_error.retryability,
            meerkat_core::error::LlmProviderErrorRetryability::NonRetryable
        );
        assert_eq!(
            provider_error.details["class"],
            serde_json::json!(QUOTA_EXHAUSTED_DETAILS_CLASS),
            "details.class stays for one release next to the dedicated kind: {:?}",
            provider_error.details
        );
        let agent_error =
            meerkat_core::error::AgentError::llm(provider, reason.clone(), err.to_string());
        assert!(!agent_error.is_recoverable());
        assert!(
            meerkat_core::retry::LlmRetryFailure::from_agent_error(&agent_error).is_none(),
            "retry authority must admit no retry plan for exhausted quota"
        );
        provider_error.clone()
    }

    #[test]
    fn quota_exhausted_projects_to_dedicated_wire_kind() {
        let err = LlmError::QuotaExhausted {
            message: "insufficient_quota".to_string(),
        };
        let LlmFailureReason::ProviderError(provider_error) = err.failure_reason() else {
            panic!("exhausted quota must keep the typed provider carrier");
        };
        assert_eq!(provider_error.kind, LlmProviderErrorKind::QuotaExhausted);
        assert!(!provider_error.is_retryable());
        let wire = serde_json::to_value(&provider_error).expect("provider error serializes");
        assert_eq!(wire["kind"], serde_json::json!("quota_exhausted"));
        assert_eq!(wire["retryability"], serde_json::json!("non_retryable"));
        assert_eq!(
            wire["details"]["class"],
            serde_json::json!(QUOTA_EXHAUSTED_DETAILS_CLASS),
            "compatibility class rides along for one release"
        );
        assert_eq!(
            wire["details"]["message"],
            serde_json::json!("insufficient_quota")
        );
        let back: meerkat_core::error::LlmProviderError =
            serde_json::from_value(wire).expect("wire kind round-trips");
        assert_eq!(back.kind, LlmProviderErrorKind::QuotaExhausted);
    }

    #[test]
    fn openai_insufficient_quota_429_terminalizes_in_one_round_trip() {
        let headers = reqwest::header::HeaderMap::new();
        let err =
            LlmError::from_http_response(429, OPENAI_INSUFFICIENT_QUOTA_BODY.to_string(), &headers);
        let provider_error = assert_quota_exhausted_is_terminal(&err, "openai");
        assert_eq!(
            provider_error.details["message"],
            serde_json::json!(OPENAI_INSUFFICIENT_QUOTA_BODY),
            "the provider body travels verbatim in details.message"
        );
        // An ordinary invalid request keeps its own kind and carries no class.
        let invalid = LlmError::InvalidRequest {
            message: "bad request".to_string(),
        };
        let LlmFailureReason::ProviderError(invalid_error) = invalid.failure_reason() else {
            panic!("invalid request keeps the provider carrier");
        };
        assert_eq!(invalid_error.kind, LlmProviderErrorKind::InvalidRequest);
        assert_ne!(invalid_error.kind, provider_error.kind);
        assert!(
            invalid_error.details.get("class").is_none(),
            "only exhausted quota carries details.class: {:?}",
            invalid_error.details
        );
    }

    #[test]
    fn provider_error_object_parses_every_documented_shape_once() {
        use serde::Deserialize as _;

        let anthropic: serde_json::Value = serde_json::from_str(ANTHROPIC_SPEND_CAP_BODY).unwrap();
        let parsed = ProviderErrorObject::deserialize(&anthropic["error"]).unwrap();
        assert!(parsed.signals_quota_exhausted());

        let openai: serde_json::Value =
            serde_json::from_str(OPENAI_INSUFFICIENT_QUOTA_BODY).unwrap();
        let parsed = ProviderErrorObject::deserialize(&openai["error"]).unwrap();
        assert!(parsed.signals_quota_exhausted());

        // Google's numeric `code` and detail list parse into the typed shape.
        let google = serde_json::json!({
            "code": 429,
            "message": "quota",
            "status": "RESOURCE_EXHAUSTED",
            "details": [
                {"@type": "type.googleapis.com/google.rpc.Help", "links": []},
                {"@type": "type.googleapis.com/google.rpc.QuotaFailure",
                 "violations": [{"quotaId": "GenerateRequestsPerDayPerProjectPerModel", "quotaValue": "250"}]}
            ]
        });
        let parsed = ProviderErrorObject::deserialize(&google).unwrap();
        assert!(parsed.signals_quota_exhausted());

        // A plain rate limit with no quota signal parses and classifies false.
        let rate_limit = serde_json::json!({"type": "rate_limit_error", "message": "slow down"});
        let parsed = ProviderErrorObject::deserialize(&rate_limit).unwrap();
        assert!(!parsed.signals_quota_exhausted());

        // A non-object error value is not an error object at all.
        assert!(ProviderErrorObject::deserialize(&serde_json::json!("string")).is_err());
    }

    #[test]
    fn openai_documented_credit_and_spend_limit_codes_are_quota_exhaustion() {
        for code in [
            "credit_balance_exhausted",
            "organization_spend_limit_exceeded",
            "project_spend_limit_exceeded",
            "organization_usage_limit_exceeded",
            "billing_hard_limit_reached",
        ] {
            let body = format!(
                r#"{{"error":{{"message":"limit reached","type":"rate_limit_error","param":null,"code":"{code}"}}}}"#
            );
            let err = LlmError::from_http_status(429, body, None);
            assert!(
                matches!(&err, LlmError::QuotaExhausted { .. }),
                "{code} must classify as exhausted quota, got {err:?}"
            );
        }
    }

    #[test]
    fn openai_transient_429_bodies_keep_the_rate_limit_class_and_hint() {
        for body in [
            r#"{"error":{"message":"Rate limit reached for gpt-5.5 in organization org-x on requests per min (RPM): Limit 500, Used 500, Requested 1. Please try again in 120ms.","type":"requests","param":null,"code":"rate_limit_exceeded"}}"#,
            r#"{"error":{"message":"Slow down","type":"rate_limit_error","param":null,"code":"slow_down"}}"#,
        ] {
            let err = LlmError::from_http_status(429, body.to_string(), Some(5000));
            assert!(
                matches!(
                    err,
                    LlmError::RateLimited {
                        retry_after_ms: Some(5000)
                    }
                ),
                "transient 429 must stay retryable with its hint: {err:?}"
            );
        }
    }

    #[test]
    fn anthropic_enforced_spend_limit_429_terminalizes_and_plain_rate_limit_retries() {
        let err = LlmError::from_http_status(429, ANTHROPIC_SPEND_CAP_BODY.to_string(), None);
        assert_quota_exhausted_is_terminal(&err, "anthropic");

        let rate_limit = r#"{"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed the rate limit for your organization (org-x) of 50 requests per minute."},"request_id":"req_x"}"#;
        let err = LlmError::from_http_status(429, rate_limit.to_string(), Some(2000));
        assert!(
            matches!(
                err,
                LlmError::RateLimited {
                    retry_after_ms: Some(2000)
                }
            ),
            "a rate_limit_error without the spend-cap code must stay retryable: {err:?}"
        );
    }

    #[test]
    fn anthropic_self_set_spend_limit_400_terminalizes_as_quota_not_invalid_request() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"You have reached your specified API usage limits. You will regain access on 2026-09-01 at 00:00 UTC."},"request_id":"req_x"}"#;
        let err = LlmError::from_http_status(400, body.to_string(), None);
        assert_quota_exhausted_is_terminal(&err, "anthropic");

        // The Anthropic stream error path hands over the message alone.
        let err = LlmError::from_http_status(
            400,
            "You have reached your specified workspace API usage limits. You will regain access on 2026-09-01 at 00:00 UTC."
                .to_string(),
            None,
        );
        assert!(matches!(err, LlmError::QuotaExhausted { .. }), "{err:?}");

        // An ordinary 400 is untouched.
        let err = LlmError::from_http_status(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages: at least one message is required"}}"#
                .to_string(),
            None,
        );
        assert!(matches!(err, LlmError::InvalidRequest { .. }), "{err:?}");
    }

    #[test]
    fn http_402_billing_error_is_quota_exhaustion() {
        let body = r#"{"type":"error","error":{"type":"billing_error","message":"There is an issue with your billing or payment information."}}"#;
        let err = LlmError::from_http_status(402, body.to_string(), None);
        assert_quota_exhausted_is_terminal(&err, "anthropic");
    }

    #[test]
    fn gemini_quota_exceeded_code_is_quota_exhaustion() {
        let body = r#"{"error":{"code":"quota_exceeded","message":"You have exceeded your daily quota."}}"#;
        let err = LlmError::from_http_status(429, body.to_string(), None);
        assert_quota_exhausted_is_terminal(&err, "gemini");
    }

    #[test]
    fn gemini_resource_exhausted_daily_or_zero_quota_terminalizes_but_per_minute_retries() {
        fn body(quota_id: &str, quota_value: &str, extra_detail: &str) -> String {
            let body = format!(
                r#"{{"error":{{"code":429,"message":"You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://ai.google.dev/gemini-api/docs/rate-limits.","status":"RESOURCE_EXHAUSTED","details":[{{"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[{{"quotaMetric":"generativelanguage.googleapis.com/generate_content_free_tier_requests","quotaId":"{quota_id}","quotaDimensions":{{"model":"gemini-3.8-flash","location":"global"}},"quotaValue":"{quota_value}"}}]}},{{"@type":"type.googleapis.com/google.rpc.Help","links":[{{"description":"Learn more about Gemini API quotas","url":"https://ai.google.dev/gemini-api/docs/rate-limits"}}]}}{extra_detail}]}}}}"#
            );
            serde_json::from_str::<serde_json::Value>(&body).expect("fixture must be valid JSON");
            body
        }

        // Daily window: resets at midnight Pacific, never inside a turn.
        let err = LlmError::from_http_status(
            429,
            body(
                "GenerateRequestsPerDayPerProjectPerModel-FreeTier",
                "250",
                "",
            ),
            None,
        );
        assert_quota_exhausted_is_terminal(&err, "gemini");

        // Zero entitlement: the project has no quota for this model at all.
        let err = LlmError::from_http_status(
            429,
            body(
                "GenerateRequestsPerMinutePerProjectPerModel-FreeTier",
                "0",
                "",
            ),
            None,
        );
        assert_quota_exhausted_is_terminal(&err, "gemini");

        // A per-minute violation with a retry delay is an ordinary rate limit.
        let err = LlmError::from_http_status(
            429,
            body(
                "GenerateRequestsPerMinutePerProjectPerModel-FreeTier",
                "15",
                r#",{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"39s"}"#,
            ),
            Some(39_000),
        );
        assert!(
            matches!(
                err,
                LlmError::RateLimited {
                    retry_after_ms: Some(39_000)
                }
            ),
            "per-minute RESOURCE_EXHAUSTED must stay retryable: {err:?}"
        );
    }

    #[test]
    fn non_json_or_unrelated_429_bodies_stay_rate_limited() {
        for body in ["", "rate limited", "<html>429 Too Many Requests</html>"] {
            let err = LlmError::from_http_status(429, body.to_string(), None);
            assert!(
                matches!(
                    err,
                    LlmError::RateLimited {
                        retry_after_ms: None
                    }
                ),
                "{body:?} must stay a rate limit: {err:?}"
            );
        }
    }
}
