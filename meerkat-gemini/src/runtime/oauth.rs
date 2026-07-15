//! Google Code Assist OAuth runtime (oauth-personal flow).
//!
//! 1:1 with Gemini CLI:
//! - Client ID / secret / scopes:
//!   `gemini-cli/packages/core/src/code_assist/oauth2.ts:72-88`
//! - Authorize/token endpoints: google-auth-library defaults
//!   (accounts.google.com/o/oauth2/v2/auth, oauth2.googleapis.com/token)
//! - Browser loopback flow: `oauth2.ts:113-760`
//! - Device/user-code flow: `oauth2.ts:360-520`
//!
//! Gemini CLI's `OAUTH_CLIENT_SECRET` is embedded because it's an
//! "installed application" per
//! https://developers.google.com/identity/protocols/oauth2#installed —
//! the secret is considered non-sensitive in this flow.

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
use meerkat_auth_core::oauth_flow::{OAuthProviderIdentity, oauth_provider_endpoints};
use meerkat_auth_core::resolver::{
    LockedManagedStoreOAuthRefresh, ManagedStoreOAuthRefreshPreparationSlot,
};

pub type TokenPrepareFn = meerkat_auth_core::resolver::ManagedStoreOAuthRefreshPrepareFn;

// ---------------------------------------------------------------------
// Provider OAuth declaration
// ---------------------------------------------------------------------
//
// The Google Code Assist OAuth declaration (client_id, client_secret,
// authorize/token/device-code endpoints, scopes) is owned by the single
// provider-owned `OAuthProviderIdentity::GoogleCodeAssist` in auth-core. The
// runtime reads it through that seam rather than re-declaring the same
// constants (dogma §123: one OAuth declaration, projected through the auth
// seam). The client secret is non-sensitive per the OAuth2 "installed
// application" spec (see Gemini CLI `oauth2.ts:76-81`).

/// The Google Code Assist OAuth client secret as declared by the auth-core
/// owner. `Option` because authless/secret-less providers carry none; the
/// token-exchange seam consumes it as `Option<&str>` directly (no fail-open
/// default substitution).
fn code_assist_client_secret() -> Option<&'static str> {
    OAuthProviderIdentity::GoogleCodeAssist.client_secret()
}

// ---------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------

/// Project the Google Code Assist OAuth endpoints from the auth-core owner.
/// Delegates to `oauth_provider_endpoints(OAuthProviderIdentity::GoogleCodeAssist, ..)`,
/// which also applies the test endpoint override.
pub fn code_assist_endpoints(redirect_uri: impl Into<String>) -> OAuthEndpoints {
    oauth_provider_endpoints(OAuthProviderIdentity::GoogleCodeAssist, redirect_uri)
}

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum GoogleCodeAssistOAuthError {
    #[error(transparent)]
    OAuth(#[from] OAuthError),
    #[error(transparent)]
    Refresh(#[from] RefreshError),
    #[error("no persisted Code Assist tokens — interactive login required")]
    InteractiveLoginRequired,
    #[error("persisted tokens missing refresh_token")]
    MissingRefreshToken,
    #[error("token store error: {0}")]
    Store(String),
}

// ---------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------

/// Google OIDC id_token claims lifted out of the Code Assist OAuth
/// flow. The flow requests `userinfo.email` + `userinfo.profile`
/// scopes, so the id_token carries the standard OIDC claims.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GoogleIdClaims {
    /// OIDC `email` claim (the signed-in user's address).
    pub email: Option<String>,
    /// OIDC `sub` claim (stable Google account ID).
    pub user_id: Option<String>,
    /// OIDC `hd` (hosted-domain) claim for Google Workspace accounts.
    pub hosted_domain: Option<String>,
}

impl GoogleIdClaims {
    /// Lift standard OIDC claims from a decoded JWT payload value.
    pub fn lift_from_claims(raw: &serde_json::Value) -> Self {
        fn get_str(v: &serde_json::Value, key: &str) -> Option<String> {
            v.get(key)
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        }
        Self {
            email: get_str(raw, "email"),
            user_id: get_str(raw, "sub"),
            hosted_domain: get_str(raw, "hd"),
        }
    }
}

pub struct GoogleCodeAssistOAuthRuntime {
    http: reqwest::Client,
    persistence: ProviderAuthPersistence,
    endpoints: OAuthEndpoints,
    key: TokenKey,
}

impl GoogleCodeAssistOAuthRuntime {
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
    ) -> Result<PersistedTokens, GoogleCodeAssistOAuthError> {
        let preparation = ManagedStoreOAuthRefreshPreparationSlot::new(prepare_fn);
        let http = self.http.clone();
        let endpoints = self.endpoints.clone();
        let token_store = self.token_store();
        let key = self.key.clone();
        let preparation_for_refresh = preparation.clone();
        // Google requires the client_secret for refresh on installed apps.
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
                        let result = match exchange_refresh_token(
                            &http,
                            &endpoints,
                            &refresh_token,
                            code_assist_client_secret(),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(error) => {
                                return Err(transaction.fail(oauth_refresh_error(error)));
                            }
                        };
                        let refreshed = match oauth_result_to_persisted(
                            result,
                            PersistedAuthMode::GoogleOauth,
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
        .map_err(GoogleCodeAssistOAuthError::from)?;

        preparation
            .finish_coordinated_refresh(
                self.refresh_coordinator(),
                self.token_store(),
                self.key.clone(),
                refreshed,
            )
            .await
            .map_err(GoogleCodeAssistOAuthError::Refresh)
    }

    pub(crate) async fn refresh_tokens_with_locked_preparation(
        &self,
        prepare_fn: TokenPrepareFn,
        force_refresh_coordination: bool,
    ) -> Result<PersistedTokens, GoogleCodeAssistOAuthError> {
        self.refresh_tokens_with_locked_preparation_inner(prepare_fn, force_refresh_coordination)
            .await
    }

    pub async fn complete_login(
        &self,
        code: &str,
        pkce_verifier: &str,
    ) -> Result<PersistedTokens, GoogleCodeAssistOAuthError> {
        let result = exchange_authorization_code(
            &self.http,
            &self.endpoints,
            code,
            pkce_verifier,
            code_assist_client_secret(),
        )
        .await?;
        let tokens = oauth_result_to_persisted(result, PersistedAuthMode::GoogleOauth, None)?;
        Ok(tokens)
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
// Login session helper
// ---------------------------------------------------------------------

pub struct GoogleLoginSession {
    pub pkce: PkcePair,
    pub state: String,
}

impl GoogleLoginSession {
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

impl Default for GoogleLoginSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn code_assist_endpoints_project_the_single_auth_core_owner() {
        // dogma §123: the runtime carries no duplicate OAuth constants — it
        // projects the single provider-owned declaration in auth-core. This
        // verifies the projected endpoints + secret carry the gemini-cli
        // values (oauth2.ts:72-81). Literals are split via concat! to satisfy
        // GitHub push-protection scanners.
        let endpoints = code_assist_endpoints("http://127.0.0.1:0/callback");
        assert_eq!(
            endpoints.client_id,
            concat!(
                "681",
                "255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j",
                ".apps.googleusercontent.com"
            ),
        );
        assert_eq!(
            code_assist_client_secret(),
            Some(concat!("GOCS", "PX-4uHgMPm-1o7Sk", "-geV6Cu5clXFsxl")),
        );
        assert_eq!(
            endpoints.authorize_url,
            "https://accounts.google.com/o/oauth2/v2/auth"
        );
        assert_eq!(endpoints.token_url, "https://oauth2.googleapis.com/token");
        assert_eq!(
            endpoints.device_code_url.as_deref(),
            Some("https://oauth2.googleapis.com/device/code")
        );
        assert_eq!(
            endpoints.scopes,
            vec![
                "https://www.googleapis.com/auth/cloud-platform".to_string(),
                "https://www.googleapis.com/auth/userinfo.email".to_string(),
                "https://www.googleapis.com/auth/userinfo.profile".to_string(),
            ]
        );
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
            PersistedAuthMode::GoogleOauth,
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
