//! Dynamic `HttpAuthorizer` implementations for cloud backends:
//! AWS (SigV4 for Bedrock), Google (ADC + metadata), Azure AD
//! (client-credentials OAuth2).
//!
//! Each authorizer acquires and caches a credential/token and adds the
//! appropriate `Authorization` (and service-specific) headers on every
//! call to [`meerkat_core::HttpAuthorizer::authorize`].

use std::sync::Arc;

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use chrono::{DateTime, Duration, Utc};
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use meerkat_core::AuthError;
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeaseHandle, AuthLeasePhase, DslTransitionError,
    LeaseKey,
};

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

    pub(crate) fn cached_token_is_fresh(
        &self,
        authorizer_label: &str,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        let snapshot = self.handle.snapshot(&self.lease_key);
        match snapshot.phase {
            Some(AuthLeasePhase::Valid) => {}
            Some(AuthLeasePhase::ReauthRequired) => {
                return Err(AuthError::Expired);
            }
            Some(
                AuthLeasePhase::Expiring | AuthLeasePhase::Refreshing | AuthLeasePhase::Released,
            )
            | None => return Ok(false),
        }

        let expected_expires_at = epoch_secs(expires_at);
        if snapshot.expires_at != Some(expected_expires_at) {
            tracing::warn!(
                authorizer = %authorizer_label,
                lease_key = %self.lease_key,
                cached_expires_at = expected_expires_at,
                lease_expires_at = snapshot.expires_at,
                "cloud authorizer cache disagrees with auth lease truth; refreshing"
            );
            return Ok(false);
        }

        Ok(token_is_fresh_at(expires_at, now))
    }

    pub(crate) fn begin_refresh(
        &self,
        authorizer_label: &str,
    ) -> Result<LeaseRefreshLifecycle, AuthError> {
        match self.handle.snapshot(&self.lease_key).phase {
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => {
                self.handle
                    .begin_refresh(&self.lease_key)
                    .map_err(|err| self.observer_error(authorizer_label, "begin_refresh", err))?;
                Ok(LeaseRefreshLifecycle::Refresh)
            }
            Some(AuthLeasePhase::ReauthRequired) => Err(AuthError::Expired),
            Some(AuthLeasePhase::Refreshing) => Err(AuthError::RefreshFailed(format!(
                "{authorizer_label} auth lease {} is already refreshing",
                self.lease_key
            ))),
            Some(AuthLeasePhase::Released) | None => Ok(LeaseRefreshLifecycle::InitialAcquire),
        }
    }

    pub(crate) fn complete_refresh(
        &self,
        authorizer_label: &str,
        lifecycle: LeaseRefreshLifecycle,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        let expires_at = epoch_secs(expires_at);
        match lifecycle {
            LeaseRefreshLifecycle::InitialAcquire => {
                self.handle
                    .acquire_lease(&self.lease_key, expires_at)
                    .map_err(|err| self.observer_error(authorizer_label, "acquire_lease", err))?;
            }
            LeaseRefreshLifecycle::Refresh => {
                self.handle
                    .complete_refresh(&self.lease_key, expires_at, epoch_secs(now))
                    .map_err(|err| {
                        self.observer_error(authorizer_label, "complete_refresh", err)
                    })?;
            }
        }
        Ok(())
    }

    pub(crate) fn refresh_failed(
        &self,
        authorizer_label: &str,
        lifecycle: LeaseRefreshLifecycle,
        permanent: bool,
    ) -> Result<(), AuthError> {
        if lifecycle == LeaseRefreshLifecycle::Refresh {
            self.handle
                .refresh_failed(&self.lease_key, permanent)
                .map_err(|err| self.observer_error(authorizer_label, "refresh_failed", err))?;
        }
        Ok(())
    }

    fn observer_error(
        &self,
        authorizer_label: &str,
        action: &'static str,
        err: DslTransitionError,
    ) -> AuthError {
        AuthError::Other(format!(
            "{authorizer_label} auth lease {action} failed for {}: {err}",
            self.lease_key
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) enum LeaseRefreshLifecycle {
    InitialAcquire,
    Refresh,
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) fn token_is_fresh_at(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    expires_at - now > Duration::seconds(AUTH_LEASE_TTL_REFRESH_WINDOW_SECS as i64)
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
fn epoch_secs(ts: DateTime<Utc>) -> u64 {
    ts.timestamp().max(0) as u64
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
