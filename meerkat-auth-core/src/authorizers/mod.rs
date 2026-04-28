//! Dynamic `HttpAuthorizer` implementations for cloud backends:
//! AWS (SigV4 for Bedrock), Google (ADC + metadata), Azure AD
//! (client-credentials OAuth2).
//!
//! Each authorizer acquires and caches a credential/token and adds the
//! appropriate `Authorization` (and service-specific) headers on every
//! call to [`meerkat_core::HttpAuthorizer::authorize`].

use std::sync::Arc;

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use chrono::{DateTime, Utc};
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use meerkat_core::handles::{AuthLeaseHandle, LeaseKey};

/// Shared closure type for env-variable lookup. Used by authorizers that
/// want to remain hermetic in tests by taking a closure rather than
/// reading `std::env::var` directly. The process-env implementation is
/// `Arc::new(|k| std::env::var(k).ok())`.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Clone)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) struct LeaseFreshnessObserver {
    handle: Arc<dyn AuthLeaseHandle>,
    lease_key: LeaseKey,
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
impl LeaseFreshnessObserver {
    pub(crate) fn new(handle: Arc<dyn AuthLeaseHandle>, lease_key: LeaseKey) -> Self {
        Self { handle, lease_key }
    }

    pub(crate) fn observe_expires_at(&self, authorizer_label: &str, expires_at: DateTime<Utc>) {
        let expires_at = expires_at.timestamp().max(0) as u64;
        if let Err(err) = self.handle.acquire_lease(&self.lease_key, expires_at) {
            tracing::warn!(
                authorizer = %authorizer_label,
                lease_key = %self.lease_key,
                expires_at,
                error = %err,
                "failed to publish cloud authorizer freshness to auth lease truth"
            );
        }
    }
}

#[cfg(feature = "aws-sigv4")]
pub mod aws;
#[cfg(feature = "azure-ad")]
pub mod azure;
#[cfg(feature = "gcp-auth")]
pub mod google;
pub mod static_bearer;

#[cfg(feature = "aws-sigv4")]
pub use aws::{AwsAuthError, AwsCredentialProvider, AwsStsAuthorizer};
#[cfg(feature = "azure-ad")]
pub use azure::{AzureAdAuthorizer, AzureAuthError, AzureClientCredentials};
#[cfg(feature = "gcp-auth")]
pub use google::{GoogleAuthAuthorizer, GoogleAuthChain, GoogleAuthError};
pub use static_bearer::StaticBearerAuthorizer;
