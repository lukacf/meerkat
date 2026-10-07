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
    oauth_endpoint_failure_observation,
};
use meerkat_core::RefreshFailureObservation;
use meerkat_core::handles::{GeneratedAuthLeaseHandle, LeaseKey};
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
    /// A token or metadata endpoint answered with a redirect, refused by
    /// its status alone (no `Location` or body is kept).
    #[error(
        "google credential endpoint answered with a redirect (status {status}); redirects are refused"
    )]
    RedirectRefused { status: u16 },
    #[error(transparent)]
    HttpClientUnavailable(#[from] crate::auth_oauth::CredentialHttpClientUnavailable),
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
            GoogleAuthError::RedirectRefused { .. } | GoogleAuthError::HttpClientUnavailable(_) => {
                AuthError::RefreshFailed(e.to_string())
            }
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
    // Private cache provenance only; freshness is decided by the
    // required AuthMachine lease snapshot.
    lease_generation: Option<u64>,
}

// --- Authorizer -------------------------------------------------------

pub struct GoogleAuthAuthorizer {
    chain: GoogleAuthChain,
    scope: String,
    cache: Arc<Mutex<Option<CachedToken>>>,
    env_lookup: EnvLookup,
    home_dir: Option<PathBuf>,
    /// Follows no redirects. A build failure is kept and every token
    /// request fails with it; nothing falls back to a default client.
    http: Result<reqwest::Client, crate::auth_oauth::CredentialHttpClientUnavailable>,
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
            http: crate::auth_oauth::credential_http_client(),
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
        handle: GeneratedAuthLeaseHandle,
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
        self.lease_observer
            .as_ref()
            .and_then(LeaseFreshnessObserver::expires_at)
    }

    fn fresh_cached_token(
        &self,
        observer: &LeaseFreshnessObserver,
        now: DateTime<Utc>,
    ) -> Result<Option<String>, AuthError> {
        let Some((access_token, expires_at, lease_generation)) = ({
            let guard = self.cache.lock();
            guard
                .as_ref()
                .map(|t| (t.access_token.clone(), t.expires_at, t.lease_generation))
        }) else {
            return Ok(None);
        };
        if observer.cached_token_is_fresh(&self.label, expires_at, lease_generation, now)? {
            return Ok(Some(access_token));
        }
        Ok(None)
    }

    async fn get_token(&self) -> Result<String, AuthError> {
        let Some(observer) = &self.lease_observer else {
            return Err(AuthError::HostOwnedUnavailable);
        };

        if let Some(access_token) = self.fresh_cached_token(observer, Utc::now())? {
            return Ok(access_token);
        }

        let _refresh_guard = self.refresh_lock.lock().await;
        if let Some(access_token) = self.fresh_cached_token(observer, Utc::now())? {
            return Ok(access_token);
        }

        let lifecycle = observer.begin_refresh(&self.label).await?;

        let mut token = match self.chain {
            GoogleAuthChain::ComputeOnly => match self.fetch_from_metadata().await {
                Ok(token) => token,
                Err(err) => {
                    observer.refresh_failed(
                        &self.label,
                        lifecycle,
                        google_refresh_failure_observation(&err),
                    )?;
                    return Err(err.into());
                }
            },
            GoogleAuthChain::Default => match self.fetch_full_chain().await {
                Ok(token) => token,
                Err(err) => {
                    observer.refresh_failed(
                        &self.label,
                        lifecycle,
                        google_refresh_failure_observation(&err),
                    )?;
                    return Err(err.into());
                }
            },
        };
        let access = token.access_token.clone();
        let expires_at = token.expires_at;
        token.lease_generation =
            Some(observer.complete_refresh(&self.label, lifecycle, expires_at, Utc::now())?);
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
            Err(err) if default_chain_keeps_metadata_error(&err) => Err(err),
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
            .http()?
            .post(&token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| GoogleAuthError::Network(e.to_string()))?;
        let status = resp.status();
        if status.is_redirection() {
            return Err(GoogleAuthError::RedirectRefused {
                status: status.as_u16(),
            });
        }
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
            .http()?
            .post(&token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| GoogleAuthError::Network(e.to_string()))?;
        let status = resp.status();
        if status.is_redirection() {
            return Err(GoogleAuthError::RedirectRefused {
                status: status.as_u16(),
            });
        }
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

    fn http(&self) -> Result<&reqwest::Client, GoogleAuthError> {
        self.http.as_ref().map_err(|error| (*error).into())
    }

    async fn fetch_from_metadata(&self) -> Result<CachedToken, GoogleAuthError> {
        let resp = self
            .http()?
            .get(self.metadata_url())
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| GoogleAuthError::Network(e.to_string()))?;
        let status = resp.status();
        if status.is_redirection() {
            return Err(GoogleAuthError::RedirectRefused {
                status: status.as_u16(),
            });
        }
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

fn google_refresh_failure_observation(err: &GoogleAuthError) -> RefreshFailureObservation {
    match err {
        GoogleAuthError::NoCredentialSource
        | GoogleAuthError::Json(_)
        | GoogleAuthError::JwtSign(_) => RefreshFailureObservation::local_credential_unusable(),
        GoogleAuthError::Io(_) | GoogleAuthError::Network(_) => {
            RefreshFailureObservation::transient()
        }
        GoogleAuthError::TokenEndpoint { status, body } => {
            oauth_endpoint_failure_observation(*status, body)
        }
        GoogleAuthError::MetadataEndpoint { .. }
        | GoogleAuthError::RedirectRefused { .. }
        | GoogleAuthError::HttpClientUnavailable(_) => RefreshFailureObservation::transient(),
    }
}

/// Whether the default chain reports a metadata-server failure as itself
/// rather than as `NoCredentialSource` (which classifies as an unusable
/// credential). A transient metadata answer, a refused redirect and an
/// unavailable client are route or host failures: collapsing them would let
/// a refresh retire a valid credential.
fn default_chain_keeps_metadata_error(err: &GoogleAuthError) -> bool {
    match err {
        GoogleAuthError::MetadataEndpoint { status, body } => {
            let _ = body;
            endpoint_failure_is_transient(*status)
        }
        GoogleAuthError::RedirectRefused { .. } | GoogleAuthError::HttpClientUnavailable(_) => true,
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// The default chain with no file credential falls back to the metadata
    /// server. A refused redirect or an unavailable client there stays typed
    /// and transient; it never becomes `NoCredentialSource`, which would
    /// retire a valid credential on refresh.
    #[tokio::test]
    async fn default_chain_keeps_route_failures_typed_and_transient() {
        use crate::auth_oauth::redirect_fixture::spawn_redirecting_endpoint;
        let home = tempfile::tempdir().unwrap();
        let (url, target_hits) = spawn_redirecting_endpoint().await;
        let default_chain = || {
            GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, Arc::new(|_| None))
                .with_home_dir(home.path())
                .with_metadata_url_override(url.clone())
        };
        let redirected = match default_chain().fetch_full_chain().await {
            Err(error) => error,
            Ok(_) => panic!("a redirect answer must not yield a token"),
        };
        assert!(
            matches!(redirected, GoogleAuthError::RedirectRefused { status: 302 }),
            "{redirected:?}"
        );
        assert_eq!(
            google_refresh_failure_observation(&redirected),
            RefreshFailureObservation::transient()
        );
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);

        let mut unavailable = default_chain();
        unavailable.http = Err(crate::auth_oauth::CredentialHttpClientUnavailable);
        let error = match unavailable.fetch_full_chain().await {
            Err(error) => error,
            Ok(_) => panic!("no client must not yield a token"),
        };
        assert!(
            matches!(error, GoogleAuthError::HttpClientUnavailable(_)),
            "{error:?}"
        );
        assert_eq!(
            google_refresh_failure_observation(&error),
            RefreshFailureObservation::transient()
        );
    }

    #[tokio::test]
    async fn user_adc_refresh_redirect_is_refused_without_following_it() {
        use crate::auth_oauth::redirect_fixture::{
            BODY_CANARY, LOCATION_CANARY, spawn_redirecting_endpoint,
        };
        let dir = tempfile::tempdir().unwrap();
        let adc = dir.path().join("adc.json");
        std::fs::write(
            &adc,
            r#"{"client_id":"client","client_secret":"client-secret","refresh_token":"refresh-secret"}"#,
        )
        .unwrap();
        let (url, target_hits) = spawn_redirecting_endpoint().await;
        let authorizer =
            GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, Arc::new(|_| None))
                .with_token_url_override(url);
        let error = match authorizer.fetch_from_user_adc(&adc).await {
            Err(error) => error,
            Ok(_) => panic!("a redirect answer must not yield a token"),
        };
        assert!(
            matches!(error, GoogleAuthError::RedirectRefused { status: 302 }),
            "{error:?}"
        );
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(LOCATION_CANARY) && !rendered.contains(BODY_CANARY));
        assert!(!rendered.contains("refresh-secret"));
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn metadata_endpoint_redirect_is_refused_without_following_it() {
        use crate::auth_oauth::redirect_fixture::{
            BODY_CANARY, LOCATION_CANARY, spawn_redirecting_endpoint,
        };
        let (url, target_hits) = spawn_redirecting_endpoint().await;
        let authorizer =
            GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::ComputeOnly, Arc::new(|_| None))
                .with_metadata_url_override(url);
        let error = match authorizer.fetch_from_metadata().await {
            Err(error) => error,
            Ok(_) => panic!("a redirect answer must not yield a token"),
        };
        assert!(
            matches!(error, GoogleAuthError::RedirectRefused { status: 302 }),
            "{error:?}"
        );
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(LOCATION_CANARY) && !rendered.contains(BODY_CANARY));
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
