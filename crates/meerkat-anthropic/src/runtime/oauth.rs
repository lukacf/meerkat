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

use chrono::Utc;
use thiserror::Error;

use meerkat_auth_core::auth_oauth::{
    OAuthEndpoints, OAuthError, OAuthTokenResult, PkcePair, exchange_authorization_code_with_state,
    exchange_refresh_token, oauth_refresh_error,
};
use meerkat_auth_core::auth_store::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, RefreshCoordinator, RefreshError,
    RefreshFn, TokenKey, TokenStore,
};
use meerkat_auth_core::resolver::{
    LockedManagedStoreOAuthRefresh, ManagedStoreOAuthRefreshPreparationSlot,
};

pub type TokenPrepareFn = meerkat_auth_core::resolver::ManagedStoreOAuthRefreshPrepareFn;

// ---------------------------------------------------------------------
// Constants (verified against claude-code/src/constants/oauth.ts)
// ---------------------------------------------------------------------

// The OAuth client id, authorize/token endpoints, and scopes are owned by
// the canonical per-provider declaration in
// `meerkat_auth_core::oauth_flow::oauth_provider_declaration` — this module
// only keeps the Anthropic-only facts that have no canonical twin there.
pub const OAUTH_BETA_HEADER_NAME: &str = "anthropic-beta";
pub const OAUTH_BETA_HEADER_VALUE: &str = "oauth-2025-04-20";

// Anthropic-only: Console OAuth → API-key provisioning endpoint and the
// manual (non-loopback) redirect contract.
pub const API_KEY_CREATE_URL: &str =
    "https://api.anthropic.com/api/oauth/claude_cli/create_api_key";
pub const MANUAL_REDIRECT_URL: &str = "https://platform.claude.com/oauth/code/callback";

/// Default redirect URI for local loopback flow — the CLI/desktop callers
/// will bind a random ephemeral port and pass the concrete URL back at
/// request time via `OAuthEndpoints.redirect_uri`.
pub const DEFAULT_LOOPBACK_REDIRECT: &str = "http://localhost:0/callback";

// ---------------------------------------------------------------------
// Endpoint constructors
// ---------------------------------------------------------------------

/// Build endpoints for the Claude.ai subscription OAuth flow.
pub fn claude_ai_endpoints(redirect_uri: impl Into<String>) -> OAuthEndpoints {
    meerkat_auth_core::oauth_flow::oauth_provider_endpoints(
        meerkat_auth_core::oauth_flow::OAuthProviderIdentity::AnthropicClaudeAi,
        redirect_uri,
    )
}

/// Build endpoints for the Console OAuth flow (used only for
/// `oauth_to_api_key` provisioning).
pub fn console_endpoints(redirect_uri: impl Into<String>) -> OAuthEndpoints {
    meerkat_auth_core::oauth_flow::oauth_provider_endpoints(
        meerkat_auth_core::oauth_flow::OAuthProviderIdentity::AnthropicConsoleApiKey,
        redirect_uri,
    )
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
    persistence: ProviderAuthPersistence,
    endpoints: OAuthEndpoints,
    key: TokenKey,
}

impl AnthropicOAuthRuntime {
    pub fn new(
        persistence: ProviderAuthPersistence,
        endpoints: OAuthEndpoints,
        key: TokenKey,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            persistence,
            endpoints,
            key,
        }
    }

    pub fn endpoints(&self) -> &OAuthEndpoints {
        &self.endpoints
    }

    pub fn key(&self) -> &TokenKey {
        &self.key
    }

    fn token_store(&self) -> Arc<dyn TokenStore> {
        self.persistence.token_store()
    }

    fn refresh_coordinator(&self) -> Arc<dyn RefreshCoordinator> {
        self.persistence.refresh_coordinator()
    }

    async fn refresh_tokens_with_locked_preparation_inner(
        &self,
        prepare_fn: TokenPrepareFn,
        force_refresh_coordination: bool,
    ) -> Result<PersistedTokens, AnthropicOAuthError> {
        let preparation = ManagedStoreOAuthRefreshPreparationSlot::new(prepare_fn);
        let http = self.http.clone();
        let endpoints = self.endpoints.clone();
        let token_store = self.token_store();
        let key = self.key.clone();
        let preparation_for_refresh = preparation.clone();
        let refresh_fn: RefreshFn = Box::new(move || {
            let http = http.clone();
            let endpoints = endpoints.clone();
            let token_store = Arc::clone(&token_store);
            let key = key.clone();
            let preparation = preparation_for_refresh.clone();
            Box::pin(async move {
                let current = token_store
                    .load(&key)
                    .await
                    .map_err(|e| RefreshError::Refresh(e.to_string()))?
                    .ok_or_else(|| {
                        RefreshError::Refresh(
                            "persisted tokens disappeared before OAuth refresh".into(),
                        )
                    })?;
                match preparation.claim_refresh_owner(current.clone()).await? {
                    LockedManagedStoreOAuthRefresh::UseCached(cached) => Ok(cached),
                    LockedManagedStoreOAuthRefresh::Refresh(transaction) => {
                        let refresh_token = match current.refresh_token.clone() {
                            Some(refresh_token) => refresh_token,
                            None => {
                                return Err(transaction.fail(RefreshError::Observed {
                                    message: "missing refresh_token".into(),
                                    observation: meerkat_core::RefreshFailureObservation::local_credential_unusable(),
                                }));
                            }
                        };
                        let result =
                            match exchange_refresh_token(&http, &endpoints, &refresh_token, None)
                                .await
                            {
                                Ok(result) => result,
                                Err(error) => {
                                    return Err(transaction.fail(oauth_refresh_error(error)));
                                }
                            };
                        let refreshed = match oauth_result_to_persisted(
                            result,
                            PersistedAuthMode::ClaudeAiOauth,
                            Some(refresh_token),
                        ) {
                            Ok(refreshed) => refreshed,
                            Err(error) => {
                                return Err(
                                    transaction.fail(RefreshError::Refresh(error.to_string()))
                                );
                            }
                        };
                        transaction.commit(refreshed).await
                    }
                }
            })
        });
        let refreshed = if force_refresh_coordination {
            self.refresh_coordinator()
                .with_forced_refresh(self.key.clone(), refresh_fn)
                .await
        } else {
            self.refresh_coordinator()
                .with_refresh(self.key.clone(), refresh_fn)
                .await
        }
        .map_err(AnthropicOAuthError::from)?;

        preparation
            .finish_coordinated_refresh(
                self.refresh_coordinator(),
                self.token_store(),
                self.key.clone(),
                refreshed,
            )
            .await
            .map_err(AnthropicOAuthError::Refresh)
    }

    pub(crate) async fn refresh_tokens_with_locked_preparation(
        &self,
        prepare_fn: TokenPrepareFn,
        force_refresh_coordination: bool,
    ) -> Result<PersistedTokens, AnthropicOAuthError> {
        self.refresh_tokens_with_locked_preparation_inner(prepare_fn, force_refresh_coordination)
            .await
    }

    /// Complete an interactive login: exchange the authorization code for
    /// tokens. Caller supplies `code + pkce_verifier` from the loopback
    /// callback and owns AuthMachine publication plus persistence.
    pub async fn complete_login(
        &self,
        code: &str,
        pkce_verifier: &str,
        state: &str,
    ) -> Result<PersistedTokens, AnthropicOAuthError> {
        let result = exchange_authorization_code_with_state(
            &self.http,
            &self.endpoints,
            code,
            pkce_verifier,
            None,
            Some(state),
        )
        .await?;
        let tokens = oauth_result_to_persisted(result, PersistedAuthMode::ClaudeAiOauth, None)?;
        Ok(tokens)
    }

    /// Console OAuth → API key provisioning. POST to `API_KEY_CREATE_URL`
    /// with `Authorization: Bearer <access_token>`; the response carries
    /// the new API key. The caller owns lifecycle admission and persistence.
    pub async fn provision_api_key_tokens(
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
        Ok(tokens)
    }

    /// Console OAuth → API key provisioning. The caller owns lifecycle
    /// admission and persistence.
    pub async fn provision_api_key(
        &self,
        access_token: &str,
    ) -> Result<PersistedTokens, AnthropicOAuthError> {
        self.provision_api_key_tokens(access_token).await
    }
}

fn oauth_result_to_persisted(
    result: OAuthTokenResult,
    mode: PersistedAuthMode,
    fallback_refresh: Option<String>,
) -> Result<PersistedTokens, OAuthError> {
    let now = Utc::now();
    let expires_at = result.expires_at_from(now)?;
    let scopes = result
        .scope
        .as_deref()
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    Ok(PersistedTokens {
        auth_mode: mode,
        primary_secret: Some(result.access_token),
        refresh_token: result.refresh_token.or(fallback_refresh),
        id_token: result.id_token,
        expires_at,
        last_refresh: Some(now),
        scopes,
        account_id: None,
        metadata: serde_json::Value::Null,
    })
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn claude_ai_endpoints_match_claude_code_loopback_contract() {
        let endpoints = claude_ai_endpoints("http://localhost:1455/callback");
        assert_eq!(endpoints.redirect_uri, "http://localhost:1455/callback");
        let declared = meerkat_auth_core::oauth_flow::oauth_provider_declaration(
            meerkat_auth_core::oauth_flow::OAuthProviderIdentity::AnthropicClaudeAi,
        );
        assert_eq!(
            endpoints.scopes,
            declared
                .scopes
                .iter()
                .map(|scope| (*scope).to_string())
                .collect::<Vec<_>>()
        );
        assert!(
            endpoints
                .extra_authorize_params
                .contains(&("code".to_string(), "true".to_string()))
        );
        assert_eq!(DEFAULT_LOOPBACK_REDIRECT, "http://localhost:0/callback");
    }

    #[test]
    fn oauth_result_to_persisted_rejects_expiry_overflow() {
        let err = oauth_result_to_persisted(
            OAuthTokenResult {
                access_token: "access-token".into(),
                refresh_token: Some("refresh-token".into()),
                id_token: None,
                expires_in_secs: Some(u64::MAX),
                scope: None,
            },
            PersistedAuthMode::ClaudeAiOauth,
            None,
        )
        .expect_err("oversized expires_in must not be persisted");

        assert!(matches!(
            err,
            OAuthError::TokenExpiryOutOfRange {
                expires_in_secs: u64::MAX
            }
        ));
    }
}
