//! OAuth 2.0 helpers.
//!
//! PKCE, authorize URL, token-exchange, loopback callback, device-code.
//! Used by per-provider OAuth runtimes in Phase 4b (OpenAI ChatGPT,
//! Anthropic Claude.ai, Google Code Assist).
//!
//! Reference-CLI parity:
//! - Codex ChatGPT OAuth: `codex-rs/login/src/server.rs`
//! - Claude Code Claude.ai OAuth: `src/services/oauth/client.ts`
//! - Gemini CLI Google OAuth: `packages/core/src/code_assist/oauth2.ts`

pub mod jwt;

#[cfg(feature = "oauth")]
pub mod callback;
#[cfg(feature = "oauth")]
pub mod device_code;
#[cfg(feature = "oauth")]
pub mod pkce;
#[cfg(feature = "oauth")]
pub mod token_exchange;

#[cfg(feature = "oauth")]
pub use callback::{LoopbackHandle, LoopbackOutcome, run_loopback_callback};
#[cfg(feature = "oauth")]
pub use device_code::{
    DeviceCodeResponse, DevicePollOutcome, poll_device_code, request_device_code,
};
#[cfg(feature = "oauth")]
pub use pkce::{PkceChallenge, PkcePair};
#[cfg(feature = "oauth")]
pub use token_exchange::{exchange_authorization_code, exchange_refresh_token};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// OAuth endpoint configuration. Each provider's concrete runtime
/// (Phase 4b) embeds one of these with static URLs and client_id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthEndpoints {
    pub client_id: String,
    /// Authorization URL (browser flow).
    pub authorize_url: String,
    /// Token exchange URL.
    pub token_url: String,
    /// Device-code endpoint (optional; only for flows that support it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_code_url: Option<String>,
    /// Redirect URI for loopback browser flow.
    pub redirect_uri: String,
    /// Scopes to request.
    pub scopes: Vec<String>,
    /// Optional "beta" or "x-" headers required during OAuth requests
    /// (e.g., Claude Code's `oauth-2025-04-20` beta header).
    #[serde(default)]
    pub extra_headers: Vec<(String, String)>,
}

impl OAuthEndpoints {
    /// Build the authorize URL with PKCE + state. Callers open this URL in
    /// the browser; the user lands on the OAuth provider's consent screen.
    #[cfg(feature = "oauth")]
    pub fn authorize_url_with_pkce(&self, pkce: &PkceChallenge, state: &str) -> String {
        let mut query = vec![
            ("response_type", "code".to_string()),
            ("client_id", self.client_id.clone()),
            ("redirect_uri", self.redirect_uri.clone()),
            ("code_challenge", pkce.code.clone()),
            ("code_challenge_method", pkce.method.to_string()),
            ("state", state.to_string()),
        ];
        if !self.scopes.is_empty() {
            query.push(("scope", self.scopes.join(" ")));
        }
        let qs = query
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        if self.authorize_url.contains('?') {
            format!("{}&{qs}", self.authorize_url)
        } else {
            format!("{}?{qs}", self.authorize_url)
        }
    }
}

/// Successful OAuth token exchange result.
#[derive(Debug, Clone)]
pub struct OAuthTokenResult {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_in_secs: Option<u64>,
    pub scope: Option<String>,
}

/// OAuth flow errors.
#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("user denied authorization")]
    UserDenied,
    #[error("callback parse error: {0}")]
    CallbackParse(String),
    #[error("token endpoint error: status={status} body={body}")]
    TokenEndpoint { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("timeout")]
    Timeout,
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("state mismatch (possible CSRF)")]
    StateMismatch,
    #[error("device flow still pending (poll again)")]
    AuthorizationPending,
    #[error("device flow slow down (increase poll interval)")]
    SlowDown,
    #[error("device flow access denied")]
    AccessDenied,
    #[error("device flow expired")]
    ExpiredToken,
}

#[cfg(feature = "oauth")]
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_includes_pkce_and_state() {
        let ep = OAuthEndpoints {
            client_id: "cid".into(),
            authorize_url: "https://example.com/oauth/authorize".into(),
            token_url: "https://example.com/oauth/token".into(),
            device_code_url: None,
            redirect_uri: "http://127.0.0.1:8777/callback".into(),
            scopes: vec!["read".into(), "write".into()],
            extra_headers: Vec::new(),
        };
        let pkce = PkcePair::generate_s256();
        let url = ep.authorize_url_with_pkce(&pkce.challenge, "state-abc");
        assert!(url.starts_with("https://example.com/oauth/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=cid"));
        assert!(url.contains(&format!("code_challenge={}", pkce.challenge.code)));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state-abc"));
        assert!(url.contains("scope=read%20write"));
        // redirect_uri is URL-encoded.
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8777%2Fcallback"));
    }

    #[test]
    fn authorize_url_preserves_existing_query() {
        let ep = OAuthEndpoints {
            client_id: "cid".into(),
            authorize_url: "https://example.com/authorize?prompt=consent".into(),
            token_url: "https://example.com/token".into(),
            device_code_url: None,
            redirect_uri: "http://localhost/cb".into(),
            scopes: vec![],
            extra_headers: Vec::new(),
        };
        let pkce = PkcePair::generate_s256();
        let url = ep.authorize_url_with_pkce(&pkce.challenge, "x");
        assert!(url.contains("prompt=consent&response_type=code"));
    }
}
