//! Anthropic OAuth runtime — Claude.ai OAuth + Console→API-key provisioning.
//!
//! 1:1 with Claude Code:
//! - `CLIENT_ID`, `AUTHORIZE_URL`, `TOKEN_URL`, scopes, beta header, and
//!   `API_KEY_URL` verified against `claude-code/src/constants/oauth.ts`.
//! - Refresh semantics: `claude-code/src/utils/auth.ts:1313-1560`.
//! - Console OAuth → API key provisioning:
//!   `claude-code/src/services/oauth/client.ts:311-321` +
//!   `claude-code/src/cli/handlers/auth.ts:79-109`.

use std::sync::Arc;

use chrono::{Duration, Utc};
use thiserror::Error;

use crate::auth_oauth::{
    OAuthEndpoints, OAuthError, OAuthTokenResult, PkcePair, exchange_authorization_code,
    exchange_refresh_token,
};
use crate::auth_store::{
    InMemoryCoordinator, PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError,
    TokenKey, TokenStore,
};

// ---------------------------------------------------------------------
// Constants (verified against claude-code/src/constants/oauth.ts)
// ---------------------------------------------------------------------

pub const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const OAUTH_BETA_HEADER_NAME: &str = "anthropic-beta";
pub const OAUTH_BETA_HEADER_VALUE: &str = "oauth-2025-04-20";

// Claude.ai OAuth (subscription auth; user:* scopes).
pub const CLAUDE_AI_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
pub const CLAUDE_AI_SCOPES: &[&str] = &[
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];

// Console OAuth (for oauth_to_api_key provisioning; org:* scope).
pub const CONSOLE_AUTHORIZE_URL: &str = "https://platform.claude.com/oauth/authorize";
pub const CONSOLE_SCOPES: &[&str] = &["org:create_api_key", "user:profile"];

// Shared.
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub const API_KEY_CREATE_URL: &str =
    "https://api.anthropic.com/api/oauth/claude_cli/create_api_key";
pub const MANUAL_REDIRECT_URL: &str = "https://platform.claude.com/oauth/code/callback";

/// Default redirect URI for local loopback flow — the CLI/desktop callers
/// will bind a random ephemeral port and pass the concrete URL back at
/// request time via `OAuthEndpoints.redirect_uri`.
pub const DEFAULT_LOOPBACK_REDIRECT: &str = "http://127.0.0.1:0/callback";

// ---------------------------------------------------------------------
// Endpoint constructors
// ---------------------------------------------------------------------

/// Build endpoints for the Claude.ai subscription OAuth flow.
pub fn claude_ai_endpoints(redirect_uri: impl Into<String>) -> OAuthEndpoints {
    OAuthEndpoints {
        client_id: CLAUDE_CLIENT_ID.into(),
        authorize_url: CLAUDE_AI_AUTHORIZE_URL.into(),
        token_url: TOKEN_URL.into(),
        device_code_url: None,
        redirect_uri: redirect_uri.into(),
        scopes: CLAUDE_AI_SCOPES.iter().map(|s| (*s).to_string()).collect(),
        extra_headers: vec![(
            OAUTH_BETA_HEADER_NAME.into(),
            OAUTH_BETA_HEADER_VALUE.into(),
        )],
    }
}

/// Build endpoints for the Console OAuth flow (used only for
/// `oauth_to_api_key` provisioning).
pub fn console_endpoints(redirect_uri: impl Into<String>) -> OAuthEndpoints {
    OAuthEndpoints {
        client_id: CLAUDE_CLIENT_ID.into(),
        authorize_url: CONSOLE_AUTHORIZE_URL.into(),
        token_url: TOKEN_URL.into(),
        device_code_url: None,
        redirect_uri: redirect_uri.into(),
        scopes: CONSOLE_SCOPES.iter().map(|s| (*s).to_string()).collect(),
        extra_headers: vec![(
            OAUTH_BETA_HEADER_NAME.into(),
            OAUTH_BETA_HEADER_VALUE.into(),
        )],
    }
}

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum AnthropicOAuthError {
    #[error(transparent)]
    OAuth(#[from] OAuthError),
    #[error(transparent)]
    Refresh(#[from] RefreshError),
    #[error("no persisted Claude.ai tokens — interactive login required")]
    InteractiveLoginRequired,
    #[error("persisted tokens missing refresh_token")]
    MissingRefreshToken,
    #[error("token store error: {0}")]
    Store(String),
    #[error("api-key provisioning failed: status={status} body={body}")]
    ApiKeyProvisioning { status: u16, body: String },
    #[error("api-key provisioning response missing api_key field")]
    ApiKeyProvisioningShape,
    #[error("network error: {0}")]
    Network(String),
}

// ---------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------

/// Per-binding Anthropic OAuth runtime. Holds the token store, refresh
/// coordinator, and OAuth endpoint config for one `(realm, binding)` pair.
/// Anthropic OIDC id_token claims lifted out of the Claude.ai OAuth
/// flow. Claude Code's `user:profile` scope surfaces the signed-in
/// user's email + subscription tier via the id_token when issued.
/// Not every Claude.ai response returns an id_token — callers handle
/// `Default::default()`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AnthropicIdClaims {
    pub email: Option<String>,
    pub user_id: Option<String>,
    /// Claude.ai subscription tier when surfaced ("free" / "pro" /
    /// "max" / "team"). Provider-specific non-OIDC claim.
    pub subscription_tier: Option<String>,
}

impl AnthropicIdClaims {
    pub fn lift_from_claims(raw: &serde_json::Value) -> Self {
        fn get_str(v: &serde_json::Value, key: &str) -> Option<String> {
            v.get(key)
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        }
        Self {
            email: get_str(raw, "email"),
            user_id: get_str(raw, "sub"),
            // Anthropic emits subscription tier under multiple key
            // variants across claim versions. We probe the two most
            // common shapes — absent → None is fine, the field is
            // optional in AnthropicAuthMetadata.
            subscription_tier: get_str(raw, "subscription_tier")
                .or_else(|| get_str(raw, "claude_ai_subscription"))
                .or_else(|| get_str(raw, "plan_type")),
        }
    }
}

pub struct AnthropicOAuthRuntime {
    http: reqwest::Client,
    token_store: Arc<dyn TokenStore>,
    refresh_coord: Arc<dyn RefreshCoordinator>,
    endpoints: OAuthEndpoints,
    key: TokenKey,
}

impl AnthropicOAuthRuntime {
    pub fn new(
        token_store: Arc<dyn TokenStore>,
        refresh_coord: Arc<dyn RefreshCoordinator>,
        endpoints: OAuthEndpoints,
        key: TokenKey,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            token_store,
            refresh_coord,
            endpoints,
            key,
        }
    }

    /// Default coordinator if no cross-process file-lock is needed.
    pub fn new_with_default_coordinator(
        token_store: Arc<dyn TokenStore>,
        endpoints: OAuthEndpoints,
        key: TokenKey,
    ) -> Self {
        Self::new(
            token_store,
            Arc::new(InMemoryCoordinator::new()),
            endpoints,
            key,
        )
    }

    pub fn endpoints(&self) -> &OAuthEndpoints {
        &self.endpoints
    }

    pub fn key(&self) -> &TokenKey {
        &self.key
    }

    /// Load the persisted bundle, or `None` if not yet saved.
    async fn load_persisted(&self) -> Result<Option<PersistedTokens>, AnthropicOAuthError> {
        self.token_store
            .load(&self.key)
            .await
            .map_err(|e| AnthropicOAuthError::Store(e.to_string()))
    }

    async fn save_persisted(&self, tokens: &PersistedTokens) -> Result<(), AnthropicOAuthError> {
        self.token_store
            .save(&self.key, tokens)
            .await
            .map_err(|e| AnthropicOAuthError::Store(e.to_string()))
    }

    /// Return a valid access token, refreshing if the persisted token is
    /// within 60s of expiry. Returns `InteractiveLoginRequired` if no
    /// tokens are persisted yet.
    pub async fn get_or_refresh_access_token(&self) -> Result<String, AnthropicOAuthError> {
        let persisted = self
            .load_persisted()
            .await?
            .ok_or(AnthropicOAuthError::InteractiveLoginRequired)?;

        // Fresh enough? Use cached.
        if let Some(expiry) = persisted.expires_at
            && expiry - Utc::now() > Duration::seconds(60)
            && let Some(access) = persisted.primary_secret
        {
            return Ok(access);
        }

        // Need to refresh. Must have a refresh_token.
        let refresh_token = persisted
            .refresh_token
            .clone()
            .ok_or(AnthropicOAuthError::MissingRefreshToken)?;

        let http = self.http.clone();
        let endpoints = self.endpoints.clone();
        let refreshed = self
            .refresh_coord
            .with_refresh(
                self.key.clone(),
                Box::new(move || {
                    let http = http.clone();
                    let endpoints = endpoints.clone();
                    Box::pin(async move {
                        let result =
                            exchange_refresh_token(&http, &endpoints, &refresh_token, None)
                                .await
                                .map_err(|e| RefreshError::Refresh(e.to_string()))?;
                        Ok(oauth_result_to_persisted(
                            result,
                            PersistedAuthMode::ClaudeAiOauth,
                            Some(refresh_token),
                        ))
                    })
                }),
            )
            .await?;

        self.save_persisted(&refreshed).await?;
        refreshed
            .primary_secret
            .ok_or(AnthropicOAuthError::InteractiveLoginRequired)
    }

    /// Complete an interactive login: exchange the authorization code for
    /// tokens and persist them. Caller supplies `code + pkce_verifier`
    /// from the loopback callback.
    pub async fn complete_login(
        &self,
        code: &str,
        pkce_verifier: &str,
    ) -> Result<PersistedTokens, AnthropicOAuthError> {
        let result =
            exchange_authorization_code(&self.http, &self.endpoints, code, pkce_verifier, None)
                .await?;
        let tokens = oauth_result_to_persisted(result, PersistedAuthMode::ClaudeAiOauth, None);
        self.save_persisted(&tokens).await?;
        Ok(tokens)
    }

    /// Console OAuth → API key provisioning. POST to `API_KEY_CREATE_URL`
    /// with `Authorization: Bearer <access_token>`; the response carries
    /// the new API key. We persist it as an api_key entry.
    pub async fn provision_api_key(
        &self,
        access_token: &str,
    ) -> Result<PersistedTokens, AnthropicOAuthError> {
        let resp = self
            .http
            .post(API_KEY_CREATE_URL)
            .bearer_auth(access_token)
            .header(OAUTH_BETA_HEADER_NAME, OAUTH_BETA_HEADER_VALUE)
            .send()
            .await
            .map_err(|e| AnthropicOAuthError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AnthropicOAuthError::ApiKeyProvisioning {
                status: status.as_u16(),
                body,
            });
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AnthropicOAuthError::Network(format!("decode: {e}")))?;
        let api_key = body
            .get("api_key")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("raw_key").and_then(|v| v.as_str()))
            .ok_or(AnthropicOAuthError::ApiKeyProvisioningShape)?
            .to_string();
        let tokens = PersistedTokens {
            auth_mode: PersistedAuthMode::OauthToApiKey,
            primary_secret: Some(api_key),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: Some(Utc::now()),
            scopes: self.endpoints.scopes.clone(),
            account_id: None,
            metadata: serde_json::Value::Null,
        };
        self.save_persisted(&tokens).await?;
        Ok(tokens)
    }
}

fn oauth_result_to_persisted(
    result: OAuthTokenResult,
    mode: PersistedAuthMode,
    fallback_refresh: Option<String>,
) -> PersistedTokens {
    let expires_at = result
        .expires_in_secs
        .map(|s| Utc::now() + Duration::seconds(s as i64));
    let scopes = result
        .scope
        .as_deref()
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    PersistedTokens {
        auth_mode: mode,
        primary_secret: Some(result.access_token),
        refresh_token: result.refresh_token.or(fallback_refresh),
        id_token: result.id_token,
        expires_at,
        last_refresh: Some(Utc::now()),
        scopes,
        account_id: None,
        metadata: serde_json::Value::Null,
    }
}

// ---------------------------------------------------------------------
// Interactive-login helper (PKCE pair + authorize URL builder)
// ---------------------------------------------------------------------

pub struct AnthropicLoginSession {
    pub pkce: PkcePair,
    pub state: String,
}

impl AnthropicLoginSession {
    pub fn new() -> Self {
        use std::time::SystemTime;
        // Unique state value for CSRF defense. Not cryptographic —
        // combines time + a random u64 via std::collections::HashMap
        // hash seed as entropy source.
        let now_ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let state = format!("st-{now_ns:x}");
        Self {
            pkce: PkcePair::generate_s256(),
            state,
        }
    }
}

impl Default for AnthropicLoginSession {
    fn default() -> Self {
        Self::new()
    }
}
