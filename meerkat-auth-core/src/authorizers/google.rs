//! Google Auth authorizer — Application Default Credentials chain plus
//! Compute Engine metadata server, implemented manually (no
//! `google-cloud-auth` crate footprint).
//!
//! Reference CLI parity:
//! - Gemini CLI user-OAuth: `packages/core/src/code_assist/oauth2.ts:113-760`
//! - Gemini CLI compute-ADC: `packages/core/src/code_assist/oauth2.ts:202-218`
//!
//! Credential sources, in order of the ADC chain:
//!   1. `GOOGLE_APPLICATION_CREDENTIALS` → service-account JSON file
//!   2. `$HOME/.config/gcloud/application_default_credentials.json` → user credentials (refresh-token flow)
//!   3. GCE metadata server (requires `GOOGLE_AUTH_METADATA_URL` in tests or real compute env)
//!
//! `GoogleAuthChain::ComputeOnly` skips (1) and (2) and only consults the
//! metadata server.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    EnvLookup, LeaseFreshnessObserver, endpoint_failure_is_transient,
    oauth_endpoint_failure_is_permanent, token_is_fresh_at,
};
use meerkat_core::handles::{AuthLeaseHandle, LeaseKey};
use meerkat_core::{AuthError, HttpAuthorizationRequest, HttpAuthorizer};

const DEFAULT_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const GOOGLE_TOKEN_URL_DEFAULT: &str = "https://oauth2.googleapis.com/token";
const METADATA_URL_DEFAULT: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleAuthChain {
    /// Full ADC chain: service account → user ADC → metadata.
    Default,
    /// Compute Engine metadata server only.
    ComputeOnly,
}

#[derive(Debug, Error)]
pub enum GoogleAuthError {
    #[error("no Google credentials available (SA file, user ADC, and metadata all failed)")]
    NoCredentialSource,
    #[error("JSON parse error: {0}")]
    Json(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("jwt sign failed: {0}")]
    JwtSign(String),
    #[error("token endpoint HTTP {status}: {body}")]
    TokenEndpoint { status: u16, body: String },
    #[error("metadata endpoint HTTP {status}: {body}")]
    MetadataEndpoint { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
}

impl From<GoogleAuthError> for AuthError {
    fn from(e: GoogleAuthError) -> Self {
        match e {
            GoogleAuthError::NoCredentialSource => AuthError::MissingSecret,
            GoogleAuthError::Io(msg) | GoogleAuthError::Network(msg) => AuthError::Io(msg),
            GoogleAuthError::TokenEndpoint { status, body } => {
                AuthError::RefreshFailed(format!("google token endpoint {status}: {body}"))
            }
            GoogleAuthError::MetadataEndpoint { status, body } => {
                AuthError::RefreshFailed(format!("google metadata endpoint {status}: {body}"))
            }
            GoogleAuthError::Json(msg) => AuthError::Other(format!("google json: {msg}")),
            GoogleAuthError::JwtSign(msg) => AuthError::Other(format!("google jwt sign: {msg}")),
        }
    }
}

// --- Credential file shapes -------------------------------------------

#[derive(Deserialize)]
struct ServiceAccountKey {
    private_key: String,
    client_email: String,
    #[serde(default = "default_token_uri")]
    token_uri: String,
}

fn default_token_uri() -> String {
    GOOGLE_TOKEN_URL_DEFAULT.into()
}

#[derive(Deserialize)]
struct UserAdcFile {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    #[serde(default = "default_token_uri")]
    token_uri: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

fn default_expires_in() -> u64 {
    3600
}

// --- Cache ------------------------------------------------------------

struct CachedToken {
    access_token: String,
    expires_at: DateTime<Utc>,
    lease_generation: Option<u64>,
}

// --- Authorizer -------------------------------------------------------

pub struct GoogleAuthAuthorizer {
    chain: GoogleAuthChain,
    scope: String,
    cache: Arc<Mutex<Option<CachedToken>>>,
    env_lookup: EnvLookup,
    home_dir: Option<PathBuf>,
    http: reqwest::Client,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    label: String,
    token_url_override: Option<String>,
    metadata_url_override: Option<String>,
    lease_observer: Option<LeaseFreshnessObserver>,
}

impl GoogleAuthAuthorizer {
    /// Construct with process env as the env_lookup source. Use
    /// [`Self::with_env_lookup`] in tests that need a hermetic env.
    ///
    /// This is the only constructor that reads `std::env::var` (inside
    /// `with_process_env`); all other paths receive an explicit
    /// `EnvLookup` closure.
    pub fn with_process_env(chain: GoogleAuthChain) -> Self {
        Self::with_env_lookup(chain, Arc::new(|k| std::env::var(k).ok()))
    }

    /// Backwards-compat alias for `with_process_env`.
    pub fn new(chain: GoogleAuthChain) -> Self {
        Self::with_process_env(chain)
    }

    pub fn with_env_lookup(chain: GoogleAuthChain, env_lookup: EnvLookup) -> Self {
        let label = match chain {
            GoogleAuthChain::Default => "google-adc".into(),
            GoogleAuthChain::ComputeOnly => "google-compute".into(),
        };
        Self {
            chain,
            scope: DEFAULT_SCOPE.into(),
            cache: Arc::new(Mutex::new(None)),
            env_lookup,
            home_dir: dirs::home_dir(),
            http: reqwest::Client::new(),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            label,
            token_url_override: None,
            metadata_url_override: None,
            lease_observer: None,
        }
    }

    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = scope.into();
        self
    }

    pub fn with_token_url_override(mut self, url: impl Into<String>) -> Self {
        self.token_url_override = Some(url.into());
        self
    }

    pub fn with_metadata_url_override(mut self, url: impl Into<String>) -> Self {
        self.metadata_url_override = Some(url.into());
        self
    }

    pub fn with_home_dir(mut self, home: impl Into<PathBuf>) -> Self {
        self.home_dir = Some(home.into());
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

    pub fn chain(&self) -> GoogleAuthChain {
        self.chain
    }

    fn token_url_default(&self) -> String {
        self.token_url_override
            .clone()
            .unwrap_or_else(|| GOOGLE_TOKEN_URL_DEFAULT.into())
    }

    fn metadata_url(&self) -> String {
        self.metadata_url_override
            .clone()
            .unwrap_or_else(|| METADATA_URL_DEFAULT.into())
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

        let mut token = match self.chain {
            GoogleAuthChain::ComputeOnly => match self.fetch_from_metadata().await {
                Ok(token) => token,
                Err(err) => {
                    if let (Some(observer), Some(lifecycle)) = (&self.lease_observer, lifecycle) {
                        observer.refresh_failed(
                            &self.label,
                            lifecycle,
                            google_refresh_failure_is_permanent(&err),
                        )?;
                    }
                    return Err(err.into());
                }
            },
            GoogleAuthChain::Default => match self.fetch_full_chain().await {
                Ok(token) => token,
                Err(err) => {
                    if let (Some(observer), Some(lifecycle)) = (&self.lease_observer, lifecycle) {
                        observer.refresh_failed(
                            &self.label,
                            lifecycle,
                            google_refresh_failure_is_permanent(&err),
                        )?;
                    }
                    return Err(err.into());
                }
            },
        };
        let access = token.access_token.clone();
        let expires_at = token.expires_at;
        if let (Some(observer), Some(lifecycle)) = (&self.lease_observer, lifecycle) {
            token.lease_generation =
                Some(observer.complete_refresh(&self.label, lifecycle, expires_at, Utc::now())?);
        }
        *self.cache.lock() = Some(token);
        Ok(access)
    }

    async fn fetch_full_chain(&self) -> Result<CachedToken, GoogleAuthError> {
        // 1. Service account file
        if let Some(path) = (self.env_lookup)("GOOGLE_APPLICATION_CREDENTIALS") {
            return self.fetch_from_service_account(&PathBuf::from(path)).await;
        }
        // 2. User ADC file
        if let Some(home) = &self.home_dir {
            let adc_path = home
                .join(".config")
                .join("gcloud")
                .join("application_default_credentials.json");
            if tokio::fs::metadata(&adc_path).await.is_ok() {
                return self.fetch_from_user_adc(&adc_path).await;
            }
        }
        // 3. Metadata server
        match self.fetch_from_metadata().await {
            Ok(t) => Ok(t),
            Err(err) if default_chain_metadata_error_is_retryable(&err) => Err(err),
            Err(_) => Err(GoogleAuthError::NoCredentialSource),
        }
    }

    async fn fetch_from_service_account(
        &self,
        path: &PathBuf,
    ) -> Result<CachedToken, GoogleAuthError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| GoogleAuthError::Io(e.to_string()))?;
        let sa: ServiceAccountKey =
            serde_json::from_slice(&bytes).map_err(|e| GoogleAuthError::Json(e.to_string()))?;

        let now = Utc::now().timestamp();
        let exp = now + 3600;
        #[derive(Serialize)]
        struct SaClaims {
            iss: String,
            scope: String,
            aud: String,
            exp: i64,
            iat: i64,
        }
        let claims = SaClaims {
            iss: sa.client_email.clone(),
            scope: self.scope.clone(),
            aud: sa.token_uri.clone(),
            exp,
            iat: now,
        };
        let key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
            .map_err(|e| GoogleAuthError::JwtSign(e.to_string()))?;
        let jwt = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key)
            .map_err(|e| GoogleAuthError::JwtSign(e.to_string()))?;

        let token_url = if self.token_url_override.is_some() {
            self.token_url_default()
        } else {
            sa.token_uri
        };
        let form = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ];
        let resp = self
            .http
            .post(&token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| GoogleAuthError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GoogleAuthError::TokenEndpoint {
                status: status.as_u16(),
                body,
            });
        }
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| GoogleAuthError::Json(e.to_string()))?;
        Ok(CachedToken {
            access_token: body.access_token,
            expires_at: Utc::now() + Duration::seconds(body.expires_in as i64),
            lease_generation: None,
        })
    }

    async fn fetch_from_user_adc(&self, path: &PathBuf) -> Result<CachedToken, GoogleAuthError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| GoogleAuthError::Io(e.to_string()))?;
        let adc: UserAdcFile =
            serde_json::from_slice(&bytes).map_err(|e| GoogleAuthError::Json(e.to_string()))?;
        let token_url = if self.token_url_override.is_some() {
            self.token_url_default()
        } else {
            adc.token_uri
        };
        let form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("client_id", adc.client_id),
            ("client_secret", adc.client_secret),
            ("refresh_token", adc.refresh_token),
        ];
        let resp = self
            .http
            .post(&token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| GoogleAuthError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GoogleAuthError::TokenEndpoint {
                status: status.as_u16(),
                body,
            });
        }
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| GoogleAuthError::Json(e.to_string()))?;
        Ok(CachedToken {
            access_token: body.access_token,
            expires_at: Utc::now() + Duration::seconds(body.expires_in as i64),
            lease_generation: None,
        })
    }

    async fn fetch_from_metadata(&self) -> Result<CachedToken, GoogleAuthError> {
        let resp = self
            .http
            .get(self.metadata_url())
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| GoogleAuthError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GoogleAuthError::MetadataEndpoint {
                status: status.as_u16(),
                body,
            });
        }
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| GoogleAuthError::Json(e.to_string()))?;
        Ok(CachedToken {
            access_token: body.access_token,
            expires_at: Utc::now() + Duration::seconds(body.expires_in as i64),
            lease_generation: None,
        })
    }
}

fn google_refresh_failure_is_permanent(err: &GoogleAuthError) -> bool {
    match err {
        GoogleAuthError::NoCredentialSource
        | GoogleAuthError::Json(_)
        | GoogleAuthError::JwtSign(_) => true,
        GoogleAuthError::Io(_) | GoogleAuthError::Network(_) => false,
        GoogleAuthError::TokenEndpoint { status, body } => {
            oauth_endpoint_failure_is_permanent(*status, body)
        }
        GoogleAuthError::MetadataEndpoint { .. } => false,
    }
}

fn default_chain_metadata_error_is_retryable(err: &GoogleAuthError) -> bool {
    match err {
        GoogleAuthError::MetadataEndpoint { status, body } => {
            endpoint_failure_is_transient(*status, body)
        }
        _ => false,
    }
}

#[async_trait]
impl HttpAuthorizer for GoogleAuthAuthorizer {
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
