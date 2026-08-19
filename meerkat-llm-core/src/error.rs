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

    #[error("Content filtered: {reason}")]
    ContentFiltered { reason: String },

    #[error("Context length exceeded: {requested} > {max}")]
    ContextLengthExceeded { max: usize, requested: usize },

    #[error("Model not found: {model}")]
    ModelNotFound { model: String },

    #[error("Invalid API key")]
    InvalidApiKey,

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

impl LlmError {
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
            403 => Self::InvalidApiKey,
            404 => Self::ModelNotFound { model: message },
            // 413 is the request-too-large class (2026-07-29 incident:
            // an over-cap transcript fails every turn terminally); attach
            // the curator-compaction recovery hint.
            413 => Self::request_too_large(message),
            429 => Self::RateLimited { retry_after_ms },
            503 => Self::ServerOverloaded,
            s if s >= 500 => Self::ServerError { status: s, message },
            s if s >= 400 => {
                if Self::body_signals_request_too_large(&message) {
                    Self::request_too_large(message)
                } else if Self::body_signals_context_exceeded(&message) {
                    let (max, requested) = Self::context_counts(&message);
                    Self::ContextLengthExceeded { max, requested }
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
}
