//! LLM client errors
//!
//! Categorized by whether they're retryable.

use meerkat_core::error::LlmFailureReason;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

/// Errors from LLM providers
///
/// Categorized by whether they're retryable.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
pub enum LlmError {
    // === Retryable Errors ===
    #[error("Rate limited, retry after {retry_after_ms:?}ms")]
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
    #[error("Unknown error: {message}")]
    Unknown { message: String },

    // === Streaming Errors ===
    #[error("Stream parsing error: {message}")]
    StreamParseError { message: String },

    #[error("Incomplete response: {message}")]
    IncompleteResponse { message: String },
}

impl LlmError {
    /// Whether this error should trigger a retry
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. }
            | Self::ServerOverloaded
            | Self::NetworkTimeout { .. }
            | Self::ConnectionReset => true,
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
    pub fn from_http_status(status: u16, message: String) -> Self {
        match status {
            401 => Self::AuthenticationFailed { message },
            403 => Self::InvalidApiKey,
            404 => Self::ModelNotFound { model: message },
            429 => Self::RateLimited {
                retry_after_ms: None,
            },
            503 => Self::ServerOverloaded,
            s if s >= 500 => Self::ServerError { status: s, message },
            s if s >= 400 => Self::InvalidRequest { message },
            _ => Self::Unknown { message },
        }
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
            Self::InvalidRequest { message } => LlmFailureReason::ProviderError(json!({
                "kind": "invalid_request",
                "retryable": false,
                "message": message,
            })),
            Self::ContentFiltered { reason } => LlmFailureReason::ProviderError(json!({
                "kind": "content_filtered",
                "retryable": false,
                "message": reason,
            })),
            Self::ServerError { status, message } => LlmFailureReason::ProviderError(json!({
                "kind": "server_error",
                "retryable": *status >= 500,
                "status": status,
                "message": message,
            })),
            Self::ServerOverloaded => LlmFailureReason::ProviderError(json!({
                "kind": "server_overloaded",
                "retryable": true,
                "message": self.to_string(),
            })),
            Self::NetworkTimeout { duration_ms } => LlmFailureReason::ProviderError(json!({
                "kind": "network_timeout",
                "retryable": true,
                "duration_ms": duration_ms,
                "message": self.to_string(),
            })),
            Self::ConnectionReset => LlmFailureReason::ProviderError(json!({
                "kind": "connection_reset",
                "retryable": true,
                "message": self.to_string(),
            })),
            Self::Unknown { message } => LlmFailureReason::ProviderError(json!({
                "kind": "unknown",
                "retryable": self.is_retryable(),
                "message": message,
            })),
            Self::StreamParseError { message } => LlmFailureReason::ProviderError(json!({
                "kind": "stream_parse_error",
                "retryable": self.is_retryable(),
                "message": message,
            })),
            Self::IncompleteResponse { message } => LlmFailureReason::ProviderError(json!({
                "kind": "incomplete_response",
                "retryable": self.is_retryable(),
                "message": message,
            })),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
            LlmError::from_http_status(401, "".to_string()),
            LlmError::AuthenticationFailed { .. }
        ));
        assert!(matches!(
            LlmError::from_http_status(429, "".to_string()),
            LlmError::RateLimited { .. }
        ));
        assert!(matches!(
            LlmError::from_http_status(503, "".to_string()),
            LlmError::ServerOverloaded
        ));
        assert!(matches!(
            LlmError::from_http_status(500, "".to_string()),
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
}
