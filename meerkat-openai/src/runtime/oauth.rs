//! OpenAI ChatGPT OAuth runtime.
//!
//! 1:1 with Codex:
//! - Client ID / issuer / scopes / redirect: `codex-rs/login/src/server.rs:51, 468-504`
//! - Token endpoint + refresh: `codex-rs/login/src/auth/manager.rs:89-90, 744, 852`
//! - JWT claims lift (`chatgpt_*`): `codex-rs/login/src/token_data.rs:71-160`
//! - ChatGPT-Account-ID + X-OpenAI-Fedramp wire headers:
//!   `codex-rs/login/src/auth/bearer_auth_provider.rs:23-38`
//!
//! Codex stores an `AuthDotJson` with `{ OPENAI_API_KEY?, tokens, last_refresh }`.
//! For the `managed_chatgpt_oauth` path we use the token bundle;
//! `api_key` mode reads `OPENAI_API_KEY` straight.

use std::sync::Arc;

use chrono::Utc;
use thiserror::Error;

use meerkat_auth_core::auth_oauth::{
    OAuthEndpoints, OAuthError, OAuthTokenResult, PkcePair, exchange_authorization_code,
    exchange_refresh_token, oauth_refresh_error,
};
use meerkat_auth_core::auth_store::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, RefreshCoordinator, RefreshError,
    RefreshFn, TokenKey, TokenStore,
};
use meerkat_auth_core::oauth_flow::{
    OAuthProviderDeclaration, OAuthProviderIdentity, oauth_provider_declaration,
    oauth_provider_endpoints,
};
use meerkat_auth_core::resolver::{
    LockedManagedStoreOAuthRefresh, ManagedStoreOAuthRefreshPreparationSlot,
};

/// The canonical ChatGPT OAuth provider declaration, owned by auth-core.
///
/// The single owner of the ChatGPT OAuth `client_id`, authorize/token
/// endpoints, scopes, and typed backend kind is the auth-core declaration for
/// [`OAuthProviderIdentity::OpenAiChatGpt`] (verified against
/// codex-rs/login/src/{auth/manager,server}.rs). This runtime reads those
/// facts from here instead of redeclaring the literals (dogma row #123). If a
/// revoke/logout flow ever lands, its endpoint belongs on the auth-core
/// declaration like its siblings, not as a runtime-local constant.
pub fn chatgpt_declaration() -> OAuthProviderDeclaration {
    oauth_provider_declaration(OAuthProviderIdentity::OpenAiChatGpt)
}

// Wire header constants are defined in `auth.rs` (unconditional module) so
// they remain available when the interactive OAuth flow is feature-gated off.
pub use meerkat_core::provider_matrix::openai_auth::{CHATGPT_ACCOUNT_HEADER, FEDRAMP_HEADER};

pub type TokenPrepareFn = meerkat_auth_core::resolver::ManagedStoreOAuthRefreshPrepareFn;

// ---------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------

pub fn chatgpt_endpoints(redirect_uri: impl Into<String>) -> OAuthEndpoints {
    // Built from the single auth-core declaration for the ChatGPT provider;
    // the test-fixture endpoint override is applied inside `endpoints()`.
    oauth_provider_endpoints(OAuthProviderIdentity::OpenAiChatGpt, redirect_uri)
}

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum OpenAiOAuthError {
    #[error(transparent)]
    OAuth(#[from] OAuthError),
    #[error(transparent)]
    Refresh(#[from] RefreshError),
    #[error("no persisted ChatGPT tokens — interactive login required")]
    InteractiveLoginRequired,
    #[error("persisted tokens missing refresh_token")]
    MissingRefreshToken,
    #[error("token store error: {0}")]
    Store(String),
}

// ---------------------------------------------------------------------
// Claims lifted from the ID token (per Codex token_data.rs:71-160)
// ---------------------------------------------------------------------

/// ChatGPT-specific JWT claims lifted out of the id_token payload.
/// Populated from `auth.openai.com` / `{account_id, plan_type, ...}` in
/// the id_token.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChatGptIdClaims {
    pub plan_type: Option<String>,
    pub user_id: Option<String>,
    pub account_id: Option<String>,
    pub is_fedramp: Option<bool>,
    pub email: Option<String>,
}

impl ChatGptIdClaims {
    /// Lift claims from a decoded JWT payload value.
    pub fn lift_from_claims(raw: &serde_json::Value) -> Self {
        // Codex claim keys live under `https://api.openai.com/auth` OR at
        // top level depending on token version. Profile email can live under
        // `https://api.openai.com/profile`. We probe all Codex-supported
        // shapes.
        let nested = raw.get("https://api.openai.com/auth");
        let profile = raw.get("https://api.openai.com/profile");
        fn get_str(v: &serde_json::Value, key: &str) -> Option<String> {
            v.get(key).and_then(|x| x.as_str()).map(ToString::to_string)
        }
        fn get_bool(v: &serde_json::Value, key: &str) -> Option<bool> {
            v.get(key).and_then(serde_json::Value::as_bool)
        }
        let nested_str = |key: &str| -> Option<String> {
            nested
                .and_then(|n| get_str(n, key))
                .or_else(|| get_str(raw, key))
        };
        let nested_bool = |key: &str| -> Option<bool> {
            nested
                .and_then(|n| get_bool(n, key))
                .or_else(|| get_bool(raw, key))
        };
        Self {
            plan_type: nested_str("chatgpt_plan_type"),
            user_id: nested_str("chatgpt_user_id").or_else(|| nested_str("user_id")),
            account_id: nested_str("chatgpt_account_id"),
            is_fedramp: nested_bool("chatgpt_account_is_fedramp"),
            email: get_str(raw, "email").or_else(|| profile.and_then(|p| get_str(p, "email"))),
        }
    }
}

// ---------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------

pub struct OpenAiOAuthRuntime {
    http: reqwest::Client,
    persistence: ProviderAuthPersistence,
    endpoints: OAuthEndpoints,
    key: TokenKey,
}

impl OpenAiOAuthRuntime {
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
    ) -> Result<PersistedTokens, OpenAiOAuthError> {
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
                        let account_id = current.account_id.clone();
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
                            PersistedAuthMode::ChatgptOauth,
                            Some(refresh_token),
                            account_id,
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
        .map_err(OpenAiOAuthError::from)?;

        preparation
            .finish_coordinated_refresh(
                self.refresh_coordinator(),
                self.token_store(),
                self.key.clone(),
                refreshed,
            )
            .await
            .map_err(OpenAiOAuthError::Refresh)
    }

    pub(crate) async fn refresh_tokens_with_locked_preparation(
        &self,
        prepare_fn: TokenPrepareFn,
        force_refresh_coordination: bool,
    ) -> Result<PersistedTokens, OpenAiOAuthError> {
        self.refresh_tokens_with_locked_preparation_inner(prepare_fn, force_refresh_coordination)
            .await
    }

    pub async fn complete_login(
        &self,
        code: &str,
        pkce_verifier: &str,
    ) -> Result<PersistedTokens, OpenAiOAuthError> {
        let result =
            exchange_authorization_code(&self.http, &self.endpoints, code, pkce_verifier, None)
                .await?;
        // Lift JWT claims from id_token if present, to populate account_id.
        let account_id = if let Some(ref id_token) = result.id_token {
            meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
                .ok()
                .and_then(|c| {
                    let claims = ChatGptIdClaims::lift_from_claims(&c.raw);
                    claims.account_id
                })
        } else {
            None
        };
        let tokens =
            oauth_result_to_persisted(result, PersistedAuthMode::ChatgptOauth, None, account_id)?;
        Ok(tokens)
    }
}

fn oauth_result_to_persisted(
    result: OAuthTokenResult,
    mode: PersistedAuthMode,
    fallback_refresh: Option<String>,
    account_id: Option<String>,
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
        account_id,
        metadata: serde_json::Value::Null,
    })
}

// ---------------------------------------------------------------------
// Interactive login session
// ---------------------------------------------------------------------

pub struct OpenAiLoginSession {
    pub pkce: PkcePair,
    pub state: String,
}

impl OpenAiLoginSession {
    pub fn new() -> Self {
        use std::time::SystemTime;
        let now_ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Self {
            pkce: PkcePair::generate_s256(),
            state: format!("st-{now_ns:x}"),
        }
    }
}

impl Default for OpenAiLoginSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn chatgpt_constants_match_codex_source() {
        // The provider facts are sourced from the single auth-core declaration.
        let declaration = chatgpt_declaration();
        assert_eq!(declaration.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
        assert_eq!(
            declaration.authorize_endpoint,
            "https://auth.openai.com/oauth/authorize"
        );
        assert_eq!(
            declaration.token_endpoint,
            "https://auth.openai.com/oauth/token"
        );
        assert!(
            declaration
                .extra_authorize_params
                .contains(&("originator", "codex_cli_rs"))
        );
        assert_eq!(
            declaration.scopes,
            &[
                "openid",
                "profile",
                "email",
                "offline_access",
                "api.connectors.read",
                "api.connectors.invoke"
            ],
        );
        assert_eq!(CHATGPT_ACCOUNT_HEADER, "ChatGPT-Account-ID");
        assert_eq!(FEDRAMP_HEADER, "X-OpenAI-Fedramp");
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
            PersistedAuthMode::ChatgptOauth,
            None,
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

    #[test]
    fn id_claims_lift_from_nested_or_top_level() {
        // Nested under `https://api.openai.com/auth`.
        let nested = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_user_id": "user_abc",
                "chatgpt_account_id": "acct_xyz",
                "chatgpt_account_is_fedramp": true,
            },
            "email": "luka@example.com",
        });
        let c = ChatGptIdClaims::lift_from_claims(&nested);
        assert_eq!(c.plan_type.as_deref(), Some("pro"));
        assert_eq!(c.user_id.as_deref(), Some("user_abc"));
        assert_eq!(c.account_id.as_deref(), Some("acct_xyz"));
        assert_eq!(c.is_fedramp, Some(true));
        assert_eq!(c.email.as_deref(), Some("luka@example.com"));

        // Top-level fallback.
        let top = serde_json::json!({
            "chatgpt_account_id": "acct_top",
            "chatgpt_plan_type": "plus",
        });
        let c = ChatGptIdClaims::lift_from_claims(&top);
        assert_eq!(c.account_id.as_deref(), Some("acct_top"));
        assert_eq!(c.plan_type.as_deref(), Some("plus"));

        let codex_shape = serde_json::json!({
            "https://api.openai.com/auth": {
                "user_id": "user_fallback",
            },
            "https://api.openai.com/profile": {
                "email": "profile@example.com",
            },
        });
        let c = ChatGptIdClaims::lift_from_claims(&codex_shape);
        assert_eq!(c.user_id.as_deref(), Some("user_fallback"));
        assert_eq!(c.email.as_deref(), Some("profile@example.com"));
    }
}
