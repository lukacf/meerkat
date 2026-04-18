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
use thiserror::Error;

use super::EnvLookup;
use meerkat_core::{AuthError, HttpAuthorizationRequest, HttpAuthorizer};

#[derive(Debug, Error)]
pub enum AwsAuthError {
    #[error("missing AWS env var: {0}")]
    MissingEnv(&'static str),
    #[error("sigv4 signing failed: {0}")]
    Sign(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}

impl From<AwsAuthError> for AuthError {
    fn from(e: AwsAuthError) -> Self {
        match e {
            AwsAuthError::MissingEnv(_) => AuthError::MissingSecret,
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
                Ok(SdkCredentials::new(ak, sk, tok, None, "meerkat-env"))
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
        }
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
    ) -> Result<Vec<(String, String)>, AwsAuthError> {
        let identity = creds.clone().into();
        let mut settings = SigningSettings::default();
        settings.signature_location = SignatureLocation::Headers;
        let params_v4 = aws_sigv4::sign::v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name(&self.service)
            .time(std::time::SystemTime::now())
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
                let creds = other.resolve_sigv4().map_err(AuthError::from)?;
                let signed = self.sign_sigv4(&creds, req).map_err(AuthError::from)?;
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
}
