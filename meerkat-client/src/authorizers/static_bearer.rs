//! Static bearer-token authorizer.
//!
//! Used by auth paths that already own a concrete bearer token string
//! and need to inject it as `Authorization: Bearer <token>` on every
//! request — notably Anthropic's Bedrock backend when the caller
//! supplies `AWS_BEARER_TOKEN_BEDROCK` directly (plan §Phase 4b.6
//! `bedrock_bearer` method) rather than STS-signing via SigV4.
//!
//! This is a trivial wrapper over [`HttpAuthorizer`] — no refresh, no
//! network — but it lets the provider client use the same Bearer-mode
//! code path as OAuth/ADC authorizers, avoiding a parallel flat-path
//! branch inside provider `build_client` implementations.

use async_trait::async_trait;

use meerkat_core::{AuthError, HttpAuthorizationRequest, HttpAuthorizer};

/// Authorizer that unconditionally sets `Authorization: Bearer <token>`.
///
/// Instantiate with [`StaticBearerAuthorizer::new`]. The token is
/// captured at construction time and cannot rotate — callers who need
/// refresh must construct a new authorizer.
pub struct StaticBearerAuthorizer {
    token: String,
    label: &'static str,
}

impl StaticBearerAuthorizer {
    /// Construct a bearer authorizer with the given static token and
    /// a label used for diagnostics.
    pub fn new(token: String, label: &'static str) -> Self {
        Self { token, label }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl HttpAuthorizer for StaticBearerAuthorizer {
    async fn authorize(&self, req: &mut HttpAuthorizationRequest<'_>) -> Result<(), AuthError> {
        req.headers.push((
            "Authorization".to_string(),
            format!("Bearer {}", self.token),
        ));
        Ok(())
    }

    fn label(&self) -> &str {
        self.label
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sets_bearer_header() {
        let authorizer = StaticBearerAuthorizer::new("tok-123".to_string(), "bedrock-bearer");
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "https://bedrock-runtime.us-east-1.amazonaws.com/model/claude/invoke",
            headers: &mut headers,
        };
        authorizer.authorize(&mut req).await.unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "Authorization");
        assert_eq!(headers[0].1, "Bearer tok-123");
        assert_eq!(authorizer.label(), "bedrock-bearer");
    }
}
