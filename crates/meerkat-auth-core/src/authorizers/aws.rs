//! AWS SigV4 authorizer for Bedrock.
//!
//! Reference CLI parity: Claude Code `src/utils/auth.ts:609-807`
//! (`refreshAndGetAwsCredentials`) + `src/services/api/client.ts:153-189`
//! (signs Bedrock requests with SigV4 after acquiring AWS credentials).
//!
//! Two credential paths supported on the authorizer:
//!   - Bearer token (`AWS_BEARER_TOKEN_BEDROCK`): produces a simple
//!     `Authorization: Bearer ...` header, no SigV4 involved.
//!   - SigV4: access_key / secret_key / optional session_token; signs every
//!     request with SigV4 using `aws-sigv4`.
//!
//! Credentials are read from either an explicit `AwsCredentialProvider::Static`
//! or from env (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
//! `AWS_SESSION_TOKEN`) via `AwsCredentialProvider::Env`.

use async_trait::async_trait;
use aws_credential_types::Credentials as SdkCredentials;
use aws_sigv4::http_request::{
    SignableBody, SignableRequest, SignatureLocation, SigningSettings, sign,
};
use chrono::{DateTime, Utc};
use thiserror::Error;

use super::{EnvLookup, LeaseFreshnessObserver};
use meerkat_core::handles::{GeneratedAuthLeaseHandle, LeaseKey};
use meerkat_core::{AuthError, HttpAuthorizationRequest, HttpAuthorizer};

#[derive(Debug, Error)]
pub enum AwsAuthError {
    #[error("missing AWS env var: {0}")]
    MissingEnv(&'static str),
    #[error("AWS_SESSION_TOKEN requires AWS_CREDENTIAL_EXPIRATION")]
    MissingSessionTokenExpiry,
    #[error("invalid AWS_CREDENTIAL_EXPIRATION: {0}")]
    InvalidCredentialExpiration(String),
    #[error("sigv4 signing failed: {0}")]
    Sign(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}

impl From<AwsAuthError> for AuthError {
    fn from(e: AwsAuthError) -> Self {
        match e {
            AwsAuthError::MissingEnv(_) => AuthError::MissingSecret,
            AwsAuthError::MissingSessionTokenExpiry => AuthError::RefreshRequired,
            AwsAuthError::InvalidCredentialExpiration(msg) => {
                AuthError::Other(format!("aws credential expiration: {msg}"))
            }
            AwsAuthError::Sign(msg) => AuthError::Other(format!("aws sigv4: {msg}")),
            AwsAuthError::InvalidUrl(msg) => AuthError::Other(format!("aws url: {msg}")),
        }
    }
}

/// AWS credential provider used by the authorizer.
#[derive(Clone)]
pub enum AwsCredentialProvider {
    Static {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
    Env(EnvLookup),
    BearerToken(String),
}

impl std::fmt::Debug for AwsCredentialProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static { .. } => f.debug_struct("Static").finish(),
            Self::Env(_) => f.debug_struct("Env").finish(),
            Self::BearerToken(_) => f.debug_struct("BearerToken").finish(),
        }
    }
}

impl AwsCredentialProvider {
    pub fn from_env<F>(env_lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        Self::Env(std::sync::Arc::new(env_lookup))
    }

    fn resolve_sigv4(&self) -> Result<SdkCredentials, AwsAuthError> {
        match self {
            Self::Static {
                access_key_id,
                secret_access_key,
                session_token,
            } => Ok(SdkCredentials::new(
                access_key_id.clone(),
                secret_access_key.clone(),
                session_token.clone(),
                None,
                "meerkat-static",
            )),
            Self::Env(lookup) => {
                let ak = lookup("AWS_ACCESS_KEY_ID")
                    .ok_or(AwsAuthError::MissingEnv("AWS_ACCESS_KEY_ID"))?;
                let sk = lookup("AWS_SECRET_ACCESS_KEY")
                    .ok_or(AwsAuthError::MissingEnv("AWS_SECRET_ACCESS_KEY"))?;
                let tok = lookup("AWS_SESSION_TOKEN");
                let expires_at = if tok.is_some() {
                    let raw = lookup("AWS_CREDENTIAL_EXPIRATION")
                        .ok_or(AwsAuthError::MissingSessionTokenExpiry)?;
                    let parsed = DateTime::parse_from_rfc3339(&raw)
                        .map_err(|err| AwsAuthError::InvalidCredentialExpiration(err.to_string()))?
                        .with_timezone(&Utc);
                    Some(std::time::SystemTime::from(parsed))
                } else {
                    None
                };
                Ok(SdkCredentials::new(ak, sk, tok, expires_at, "meerkat-env"))
            }
            Self::BearerToken(_) => Err(AwsAuthError::MissingEnv("<bearer mode active>")),
        }
    }
}

pub struct AwsStsAuthorizer {
    region: String,
    service: String,
    provider: AwsCredentialProvider,
    label: String,
    /// Explicit SigV4 freshness authority. Runtime-backed provider resolution
    /// upgrades this to the generated AuthMachine observer; direct public
    /// construction remains a standalone user-owned credential authority so the
    /// API stays usable outside Meerkat runtime sessions.
    freshness_authority: AwsSigV4FreshnessAuthority,
}

enum AwsSigV4FreshnessAuthority {
    StandaloneDirect,
    RuntimeLease(LeaseFreshnessObserver),
}

impl AwsSigV4FreshnessAuthority {
    fn ensure_valid_for_signing(
        &self,
        label: &str,
        now: DateTime<Utc>,
        credential_expiry: Option<DateTime<Utc>>,
    ) -> Result<(), AuthError> {
        match self {
            Self::RuntimeLease(observer) => {
                observer.ensure_valid_for_signing(label, now, credential_expiry)
            }
            Self::StandaloneDirect => {
                if let Some(expires_at) = credential_expiry
                    && expires_at <= now
                {
                    return Err(AuthError::Expired);
                }
                Ok(())
            }
        }
    }

    fn expires_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::RuntimeLease(observer) => observer.expires_at(),
            Self::StandaloneDirect => None,
        }
    }
}

impl AwsStsAuthorizer {
    /// SigV4 authorizer for the given AWS service (typically `"bedrock"`).
    pub fn new(region: impl Into<String>, provider: AwsCredentialProvider) -> Self {
        Self::with_service(region, "bedrock", provider)
    }

    pub fn with_service(
        region: impl Into<String>,
        service: impl Into<String>,
        provider: AwsCredentialProvider,
    ) -> Self {
        let region = region.into();
        let service = service.into();
        let label = format!("aws-sigv4({service}/{region})");
        Self {
            region,
            service,
            provider,
            label,
            freshness_authority: AwsSigV4FreshnessAuthority::StandaloneDirect,
        }
    }

    /// Wire the per-binding AuthMachine lease so the authorizer consults
    /// credential-use freshness before signing. Mirrors the Google/Azure
    /// authorizers' `with_auth_lease_observer`.
    pub fn with_auth_lease_observer(
        mut self,
        handle: GeneratedAuthLeaseHandle,
        lease_key: LeaseKey,
    ) -> Self {
        self.freshness_authority = AwsSigV4FreshnessAuthority::RuntimeLease(
            LeaseFreshnessObserver::new(handle, lease_key),
        );
        self
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    fn sign_sigv4(
        &self,
        creds: &SdkCredentials,
        req: &HttpAuthorizationRequest<'_>,
        signing_time: DateTime<Utc>,
    ) -> Result<Vec<(String, String)>, AwsAuthError> {
        let identity = creds.clone().into();
        let mut settings = SigningSettings::default();
        settings.signature_location = SignatureLocation::Headers;
        let params_v4 = aws_sigv4::sign::v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name(&self.service)
            // Sign against the same instant the lease freshness verdict was
            // taken at, not an independent buried wall-clock read.
            .time(std::time::SystemTime::from(signing_time))
            .settings(settings)
            .build()
            .map_err(|e| AwsAuthError::Sign(e.to_string()))?;
        let params: aws_sigv4::http_request::SigningParams<'_> = params_v4.into();

        let headers_snapshot: Vec<(String, String)> = req
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let signable = SignableRequest::new(
            req.method,
            req.url,
            headers_snapshot
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
            SignableBody::UnsignedPayload,
        )
        .map_err(|e| AwsAuthError::Sign(e.to_string()))?;

        let (instructions, _signature) = sign(signable, &params)
            .map_err(|e| AwsAuthError::Sign(e.to_string()))?
            .into_parts();

        // Extract header instructions back into our (name, value) vec.
        let (header_adds, _query_params) = instructions.into_parts();
        let mut signed_headers = Vec::new();
        for header in header_adds {
            signed_headers.push((header.name().to_string(), header.value().to_string()));
        }
        Ok(signed_headers)
    }
}

#[async_trait]
impl HttpAuthorizer for AwsStsAuthorizer {
    async fn authorize(&self, req: &mut HttpAuthorizationRequest<'_>) -> Result<(), AuthError> {
        match &self.provider {
            AwsCredentialProvider::BearerToken(tok) => {
                req.headers
                    .push(("Authorization".into(), format!("Bearer {tok}")));
                Ok(())
            }
            other => {
                // Single authoritative `now`: the same instant feeds the
                // freshness verdict and the SigV4 signing time. Runtime-backed
                // authorizers delegate that verdict to the AuthMachine lease;
                // direct public authorizers use their explicit standalone
                // credential owner and fail closed only when credential expiry
                // metadata is present and already expired.
                let now = Utc::now();
                let creds = other.resolve_sigv4().map_err(AuthError::from)?;
                let credential_expiry = creds.expiry().map(DateTime::<Utc>::from);
                self.freshness_authority.ensure_valid_for_signing(
                    &self.label,
                    now,
                    credential_expiry,
                )?;
                let signed = self.sign_sigv4(&creds, req, now).map_err(AuthError::from)?;
                for (k, v) in signed {
                    req.headers.push((k, v));
                }
                Ok(())
            }
        }
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.freshness_authority.expires_at()
    }
}
