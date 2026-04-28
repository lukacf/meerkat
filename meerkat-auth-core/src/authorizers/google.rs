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

use super::{EnvLookup, LeaseFreshnessObserver};
use meerkat_core::handles::{AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeaseHandle, LeaseKey};
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
            GoogleAuthError::TokenEndpoint { status, body }
            | GoogleAuthError::MetadataEndpoint { status, body } => {
                AuthError::RefreshFailed(format!("google token endpoint {status}: {body}"))
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
}

// --- Authorizer -------------------------------------------------------

pub struct GoogleAuthAuthorizer {
    chain: GoogleAuthChain,
    scope: String,
    cache: Arc<Mutex<Option<CachedToken>>>,
    env_lookup: EnvLookup,
    home_dir: Option<PathBuf>,
    http: reqwest::Client,
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
        self.cache.lock().as_ref().map(|token| token.expires_at)
    }

    fn publish_expires_at(&self, expires_at: DateTime<Utc>) {
        if let Some(observer) = &self.lease_observer {
            observer.observe_expires_at(&self.label, expires_at);
        }
    }

    async fn get_token(&self) -> Result<String, AuthError> {
        {
            let guard = self.cache.lock();
            if let Some(t) = guard.as_ref()
                && t.expires_at - Utc::now()
                    > Duration::seconds(AUTH_LEASE_TTL_REFRESH_WINDOW_SECS as i64)
            {
                return Ok(t.access_token.clone());
            }
        }

        let token = match self.chain {
            GoogleAuthChain::ComputeOnly => self.fetch_from_metadata().await?,
            GoogleAuthChain::Default => self.fetch_full_chain().await?,
        };
        let access = token.access_token.clone();
        let expires_at = token.expires_at;
        *self.cache.lock() = Some(token);
        self.publish_expires_at(expires_at);
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
        })
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
