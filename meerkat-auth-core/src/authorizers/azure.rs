//! Azure AD authorizer — client-credentials OAuth2 flow against
//! `https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token`.
//!
//! Reference CLI parity: Claude Code
//! `src/services/api/client.ts:191-219` uses Azure
//! `DefaultAzureCredential` + `getBearerTokenProvider(...)` against scope
//! `https://cognitiveservices.azure.com/.default`. We implement the core
//! client-credentials path manually (same protocol, no SDK footprint).
//!
//! Credential sources, in order:
//!   1. Explicit `AzureClientCredentials { tenant, client_id, client_secret }`
//!   2. Environment variables `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`
//!   3. `AZURE_AUTHORITY_HOST` override (defaults to `login.microsoftonline.com`)
//!
//! Tokens are cached in memory until 60s before expiry; refresh is
//! single-flight via an internal mutex.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::Deserialize;
use thiserror::Error;

use super::{LeaseFreshnessObserver, oauth_endpoint_failure_is_permanent, token_is_fresh_at};
use meerkat_core::handles::{AuthLeaseHandle, LeaseKey};
use meerkat_core::{AuthError, HttpAuthorizationRequest, HttpAuthorizer};

const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com";

#[derive(Clone, Debug)]
pub struct AzureClientCredentials {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub authority_host: String,
}

impl AzureClientCredentials {
    /// Read credentials from `AZURE_{TENANT_ID,CLIENT_ID,CLIENT_SECRET}`.
    pub fn from_env<F>(env_lookup: F) -> Result<Self, AzureAuthError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let tenant_id =
            env_lookup("AZURE_TENANT_ID").ok_or(AzureAuthError::MissingEnv("AZURE_TENANT_ID"))?;
        let client_id =
            env_lookup("AZURE_CLIENT_ID").ok_or(AzureAuthError::MissingEnv("AZURE_CLIENT_ID"))?;
        let client_secret = env_lookup("AZURE_CLIENT_SECRET")
            .ok_or(AzureAuthError::MissingEnv("AZURE_CLIENT_SECRET"))?;
        let authority_host =
            env_lookup("AZURE_AUTHORITY_HOST").unwrap_or_else(|| DEFAULT_AUTHORITY.into());
        Ok(Self {
            tenant_id,
            client_id,
            client_secret,
            authority_host,
        })
    }
}

#[derive(Debug, Error)]
pub enum AzureAuthError {
    #[error("required Azure env var missing: {0}")]
    MissingEnv(&'static str),
    #[error("token endpoint HTTP {status}: {body}")]
    TokenEndpoint { status: u16, body: String },
    #[error("network error calling token endpoint: {0}")]
    Network(String),
    #[error("invalid token response: {0}")]
    InvalidResponse(String),
}

impl From<AzureAuthError> for AuthError {
    fn from(e: AzureAuthError) -> Self {
        match e {
            AzureAuthError::MissingEnv(_) => AuthError::MissingSecret,
            AzureAuthError::Network(msg) => AuthError::Io(msg),
            AzureAuthError::TokenEndpoint { status, body } => {
                AuthError::RefreshFailed(format!("azure token endpoint returned {status}: {body}"))
            }
            AzureAuthError::InvalidResponse(msg) => AuthError::Other(format!("azure: {msg}")),
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

struct CachedToken {
    access_token: String,
    expires_at: DateTime<Utc>,
    lease_generation: Option<u64>,
}

pub struct AzureAdAuthorizer {
    scope: String,
    creds: AzureClientCredentials,
    http: reqwest::Client,
    cache: Arc<Mutex<Option<CachedToken>>>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    label: String,
    token_url_override: Option<String>,
    lease_observer: Option<LeaseFreshnessObserver>,
}

impl AzureAdAuthorizer {
    pub fn new(scope: impl Into<String>, creds: AzureClientCredentials) -> Self {
        let scope = scope.into();
        let label = format!("azure-ad({})", &scope);
        Self {
            scope,
            creds,
            http: reqwest::Client::new(),
            cache: Arc::new(Mutex::new(None)),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            label,
            token_url_override: None,
            lease_observer: None,
        }
    }

    /// Override the token-exchange URL. Used by tests pointing at a local
    /// mock server. Production code should not call this.
    pub fn with_token_url_override(mut self, url: impl Into<String>) -> Self {
        self.token_url_override = Some(url.into());
        self
    }

    pub fn with_auth_lease_observer(
        mut self,
        handle: Arc<dyn AuthLeaseHandle>,
        lease_key: LeaseKey,
    ) -> Self {
        self.lease_observer = Some(LeaseFreshnessObserver::new(handle, lease_key));
        self
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    fn token_url(&self) -> String {
        if let Some(url) = &self.token_url_override {
            return url.clone();
        }
        format!(
            "{}/{}/oauth2/v2.0/token",
            self.creds.authority_host.trim_end_matches('/'),
            self.creds.tenant_id,
        )
    }

    async fn fetch_token(&self) -> Result<CachedToken, AzureAuthError> {
        let form = vec![
            ("grant_type", "client_credentials".to_string()),
            ("client_id", self.creds.client_id.clone()),
            ("client_secret", self.creds.client_secret.clone()),
            ("scope", self.scope.clone()),
        ];
        let resp = self
            .http
            .post(self.token_url())
            .form(&form)
            .send()
            .await
            .map_err(|e| AzureAuthError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AzureAuthError::TokenEndpoint {
                status: status.as_u16(),
                body,
            });
        }
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| AzureAuthError::InvalidResponse(e.to_string()))?;
        let expires_at = Utc::now() + Duration::seconds(body.expires_in as i64);
        Ok(CachedToken {
            access_token: body.access_token,
            expires_at,
            lease_generation: None,
        })
    }

    fn cached_expires_at(&self) -> Option<DateTime<Utc>> {
        if let Some(observer) = &self.lease_observer {
            return observer.expires_at();
        }
        self.cache.lock().as_ref().map(|token| token.expires_at)
    }

    fn fresh_cached_token(&self, now: DateTime<Utc>) -> Result<Option<String>, AuthError> {
        if let Some((access_token, expires_at, lease_generation)) = {
            let guard = self.cache.lock();
            guard
                .as_ref()
                .map(|t| (t.access_token.clone(), t.expires_at, t.lease_generation))
        } {
            let fresh = if let Some(observer) = &self.lease_observer {
                observer.cached_token_is_fresh(&self.label, expires_at, lease_generation, now)?
            } else {
                token_is_fresh_at(expires_at, now)
            };
            if fresh {
                return Ok(Some(access_token));
            }
        }
        Ok(None)
    }

    async fn get_token(&self) -> Result<String, AuthError> {
        // Check cache without taking the refresh path.
        if let Some(access_token) = self.fresh_cached_token(Utc::now())? {
            return Ok(access_token);
        }

        let _refresh_guard = self.refresh_lock.lock().await;
        if let Some(access_token) = self.fresh_cached_token(Utc::now())? {
            return Ok(access_token);
        }

        let lifecycle = if let Some(observer) = &self.lease_observer {
            Some(observer.begin_refresh(&self.label).await?)
        } else {
            None
        };

        // Miss — fetch a fresh token.
        let mut new_token = match self.fetch_token().await {
            Ok(token) => token,
            Err(err) => {
                if let (Some(observer), Some(lifecycle)) = (&self.lease_observer, lifecycle) {
                    observer.refresh_failed(
                        &self.label,
                        lifecycle,
                        azure_refresh_failure_is_permanent(&err),
                    )?;
                }
                return Err(err.into());
            }
        };
        let access = new_token.access_token.clone();
        let expires_at = new_token.expires_at;
        if let (Some(observer), Some(lifecycle)) = (&self.lease_observer, lifecycle) {
            new_token.lease_generation =
                Some(observer.complete_refresh(&self.label, lifecycle, expires_at, Utc::now())?);
        }
        *self.cache.lock() = Some(new_token);
        Ok(access)
    }
}

fn azure_refresh_failure_is_permanent(err: &AzureAuthError) -> bool {
    match err {
        AzureAuthError::MissingEnv(_) | AzureAuthError::InvalidResponse(_) => true,
        AzureAuthError::Network(_) => false,
        AzureAuthError::TokenEndpoint { status, body } => {
            oauth_endpoint_failure_is_permanent(*status, body)
        }
    }
}

#[async_trait]
impl HttpAuthorizer for AzureAdAuthorizer {
    async fn authorize(&self, req: &mut HttpAuthorizationRequest<'_>) -> Result<(), AuthError> {
        let token = self.get_token().await?;
        req.headers
            .push(("Authorization".into(), format!("Bearer {token}")));
        Ok(())
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.cached_expires_at()
    }
}
