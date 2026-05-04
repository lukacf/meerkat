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
pub use callback::{
    LoopbackBinding, LoopbackHandle, LoopbackOutcome, bind_loopback_callback,
    bind_loopback_callback_with_redirect, run_loopback_callback,
};
#[cfg(feature = "oauth")]
pub use device_code::{
    DeviceCodeResponse, DevicePollOutcome, poll_device_code, request_device_code,
};
#[cfg(feature = "oauth")]
pub use pkce::{PkceChallenge, PkcePair};
#[cfg(feature = "oauth")]
pub use token_exchange::{
    exchange_authorization_code, exchange_authorization_code_with_state, exchange_refresh_token,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Encoding expected by the provider token endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OAuthTokenRequestFormat {
    #[default]
    FormUrlEncoded,
    Json,
}

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
    /// Provider-specific authorize-query params that are part of the public
    /// login contract, not token-exchange headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_authorize_params: Vec<(String, String)>,
    /// Provider-specific token request body encoding.
    #[serde(default)]
    pub token_request_format: OAuthTokenRequestFormat,
    /// Some providers require the browser callback state to be echoed in the
    /// authorization-code token exchange request.
    #[serde(default)]
    pub include_state_in_token_exchange: bool,
    /// Scopes to request during refresh-token exchange.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refresh_scopes: Vec<String>,
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
        query.extend(
            self.extra_authorize_params
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone())),
        );
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

impl OAuthTokenResult {
    pub fn expires_at_from(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, OAuthError> {
        let Some(expires_in_secs) = self.expires_in_secs else {
            return Ok(None);
        };
        let signed_seconds = i64::try_from(expires_in_secs)
            .map_err(|_| OAuthError::TokenExpiryOutOfRange { expires_in_secs })?;
        let lifetime = chrono::Duration::try_seconds(signed_seconds)
            .ok_or(OAuthError::TokenExpiryOutOfRange { expires_in_secs })?;
        now.checked_add_signed(lifetime)
            .map(Some)
            .ok_or(OAuthError::TokenExpiryOutOfRange { expires_in_secs })
    }
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
    #[error("token expires_in is out of range: {expires_in_secs}")]
    TokenExpiryOutOfRange { expires_in_secs: u64 },
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
            extra_authorize_params: Vec::new(),
            token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
            include_state_in_token_exchange: false,
            refresh_scopes: Vec::new(),
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
            extra_authorize_params: Vec::new(),
            token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
            include_state_in_token_exchange: false,
            refresh_scopes: Vec::new(),
            extra_headers: Vec::new(),
        };
        let pkce = PkcePair::generate_s256();
        let url = ep.authorize_url_with_pkce(&pkce.challenge, "x");
        assert!(url.contains("prompt=consent&response_type=code"));
    }

    #[test]
    fn authorize_url_includes_extra_authorize_params() {
        let ep = OAuthEndpoints {
            client_id: "cid".into(),
            authorize_url: "https://example.com/oauth/authorize".into(),
            token_url: "https://example.com/oauth/token".into(),
            device_code_url: None,
            redirect_uri: "http://localhost:1455/auth/callback".into(),
            scopes: vec!["openid".into()],
            extra_authorize_params: vec![
                ("id_token_add_organizations".into(), "true".into()),
                ("codex_cli_simplified_flow".into(), "true".into()),
                ("originator".into(), "codex_cli_rs".into()),
            ],
            token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
            include_state_in_token_exchange: false,
            refresh_scopes: Vec::new(),
            extra_headers: Vec::new(),
        };
        let pkce = PkcePair::generate_s256();
        let url = ep.authorize_url_with_pkce(&pkce.challenge, "state-abc");

        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains("originator=codex_cli_rs"));
    }

    fn token_result(expires_in_secs: Option<u64>) -> OAuthTokenResult {
        OAuthTokenResult {
            access_token: "access-token".to_string(),
            refresh_token: None,
            id_token: None,
            expires_in_secs,
            scope: None,
        }
    }

    #[test]
    fn token_expiry_rejects_lifetime_that_cannot_fit_signed_duration() {
        let result = token_result(Some(u64::MAX));
        let err = result
            .expires_at_from(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap())
            .expect_err("oversized expires_in must not wrap negative");

        assert!(matches!(
            err,
            OAuthError::TokenExpiryOutOfRange {
                expires_in_secs: u64::MAX
            }
        ));
    }

    #[test]
    fn token_expiry_rejects_timestamp_overflow() {
        let result = token_result(Some(1));
        let err = result
            .expires_at_from(chrono::DateTime::<chrono::Utc>::MAX_UTC)
            .expect_err("expires_in must not overflow DateTime bounds");

        assert!(matches!(
            err,
            OAuthError::TokenExpiryOutOfRange { expires_in_secs: 1 }
        ));
    }
}
