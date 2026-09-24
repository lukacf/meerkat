//! Webhook authentication for the REST event endpoint.
//!
//! Supports two modes:
//! - `None`: no verification (suitable for localhost-only or dev)
//! - `SharedSecret`: constant-time comparison of `X-Webhook-Secret` header
//!   against the `RKAT_WEBHOOK_SECRET` environment variable
//!
//! HMAC signature verification is deferred to v2.

use axum::http::HeaderMap;
use subtle::ConstantTimeEq;

/// Webhook authentication mode.
///
/// The secret is resolved once from the environment at construction time
/// (via `from_env()`), not on every request. This avoids TOCTOU issues.
#[derive(Debug, Clone, Default)]
pub enum WebhookAuth {
    /// No authentication — accept all requests.
    #[default]
    None,
    /// Shared secret: constant-time compare `X-Webhook-Secret` header.
    SharedSecret {
        /// The expected secret value (read from env at construction time).
        secret: String,
    },
}

impl WebhookAuth {
    /// Resolve webhook auth from the `RKAT_WEBHOOK_SECRET` environment variable.
    ///
    /// - If set and non-empty: `SharedSecret` mode with the value
    /// - Otherwise: `None` mode (no authentication)
    pub fn from_env() -> Self {
        match std::env::var("RKAT_WEBHOOK_SECRET") {
            Ok(s) if !s.is_empty() => Self::SharedSecret { secret: s },
            _ => Self::None,
        }
    }
}

/// Verify webhook authentication.
///
/// Returns `Ok(())` if the request is authorized, `Err(message)` otherwise.
pub fn verify_webhook(headers: &HeaderMap, auth: &WebhookAuth) -> Result<(), &'static str> {
    match auth {
        WebhookAuth::None => Ok(()),
        WebhookAuth::SharedSecret { secret } => {
            let provided = headers
                .get("x-webhook-secret")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            // Constant-time comparison via `subtle::ConstantTimeEq`
            if secret.len() == provided.len()
                && bool::from(secret.as_bytes().ct_eq(provided.as_bytes()))
            {
                Ok(())
            } else {
                Err("invalid webhook secret")
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn auth_secret(s: &str) -> WebhookAuth {
        WebhookAuth::SharedSecret {
            secret: s.to_string(),
        }
    }

    #[test]
    fn test_webhook_auth_none_always_passes() {
        let headers = HeaderMap::new();
        assert!(verify_webhook(&headers, &WebhookAuth::None).is_ok());
    }

    #[test]
    fn test_webhook_auth_shared_secret_correct() {
        let mut headers = HeaderMap::new();
        headers.insert("x-webhook-secret", "test-secret-123".parse().unwrap());
        assert!(verify_webhook(&headers, &auth_secret("test-secret-123")).is_ok());
    }

    #[test]
    fn test_webhook_auth_shared_secret_wrong() {
        let mut headers = HeaderMap::new();
        headers.insert("x-webhook-secret", "wrong-secret".parse().unwrap());
        assert!(verify_webhook(&headers, &auth_secret("test-secret-123")).is_err());
    }

    #[test]
    fn test_webhook_auth_shared_secret_missing_header() {
        let headers = HeaderMap::new();
        assert!(verify_webhook(&headers, &auth_secret("test-secret-123")).is_err());
    }

    #[test]
    fn test_webhook_auth_shared_secret_different_length() {
        let mut headers = HeaderMap::new();
        headers.insert("x-webhook-secret", "short".parse().unwrap());
        assert!(verify_webhook(&headers, &auth_secret("much-longer-secret")).is_err());
    }
}
