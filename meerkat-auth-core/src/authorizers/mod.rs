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
const AUTH_LEASE_REFRESH_WAIT_POLL_MS: u64 = 10;
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
const AUTH_LEASE_REFRESH_WAIT_TIMEOUT_SECS: u64 = 30;

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
impl LeaseFreshnessObserver {
    pub(crate) fn new(handle: Arc<dyn AuthLeaseHandle>, lease_key: LeaseKey) -> Self {
        Self { handle, lease_key }
    }

    pub(crate) fn cached_token_is_fresh(
        &self,
        authorizer_label: &str,
        expires_at: DateTime<Utc>,
        lease_generation: Option<u64>,
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

        let Some(lease_generation) = lease_generation else {
            tracing::warn!(
                authorizer = %authorizer_label,
                lease_key = %self.lease_key,
                snapshot_generation = snapshot.generation,
                "cloud authorizer cache has no auth lease generation; refreshing"
            );
            return Ok(false);
        };

        if snapshot.generation != lease_generation {
            tracing::warn!(
                authorizer = %authorizer_label,
                lease_key = %self.lease_key,
                cached_lease_generation = lease_generation,
                snapshot_generation = snapshot.generation,
                "cloud authorizer cache belongs to an older auth lease generation; refreshing"
            );
            return Ok(false);
        }

        let expected_expires_at = epoch_secs(expires_at);
        let Some(lease_expires_at) = snapshot.expires_at else {
            tracing::warn!(
                authorizer = %authorizer_label,
                lease_key = %self.lease_key,
                cached_expires_at = expected_expires_at,
                snapshot_generation = snapshot.generation,
                "cloud authorizer cache has no auth lease expiry truth; refreshing"
            );
            return Ok(false);
        };
        if lease_expires_at != expected_expires_at {
            tracing::warn!(
                authorizer = %authorizer_label,
                lease_key = %self.lease_key,
                cached_expires_at = expected_expires_at,
                lease_expires_at,
                snapshot_generation = snapshot.generation,
                "cloud authorizer cache disagrees with auth lease truth; refreshing"
            );
            return Ok(false);
        }

        Ok(lease_epoch_secs_is_fresh_at(lease_expires_at, now))
    }

    pub(crate) fn expires_at(&self) -> Option<DateTime<Utc>> {
        let snapshot = self.handle.snapshot(&self.lease_key);
        snapshot
            .expires_at
            .and_then(|secs| i64::try_from(secs).ok())
            .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
    }

    pub(crate) async fn begin_refresh(
        &self,
        authorizer_label: &str,
    ) -> Result<LeaseRefreshLifecycle, AuthError> {
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(AUTH_LEASE_REFRESH_WAIT_TIMEOUT_SECS);
        loop {
            match self.try_begin_refresh(authorizer_label)? {
                LeaseRefreshStart::Started(lifecycle) => return Ok(lifecycle),
                LeaseRefreshStart::WaitForInFlight => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(AuthError::RefreshFailed(format!(
                            "{authorizer_label} auth lease {} remained refreshing for {AUTH_LEASE_REFRESH_WAIT_TIMEOUT_SECS}s",
                            self.lease_key
                        )));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(
                        AUTH_LEASE_REFRESH_WAIT_POLL_MS,
                    ))
                    .await;
                }
            }
        }
    }

    fn try_begin_refresh(&self, authorizer_label: &str) -> Result<LeaseRefreshStart, AuthError> {
        match self.handle.snapshot(&self.lease_key).phase {
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => {
                self.handle
                    .begin_refresh(&self.lease_key)
                    .map_err(|err| self.observer_error(authorizer_label, "begin_refresh", err))?;
                Ok(LeaseRefreshStart::Started(LeaseRefreshLifecycle::Refresh))
            }
            Some(AuthLeasePhase::ReauthRequired) => Err(AuthError::Expired),
            Some(AuthLeasePhase::Refreshing) => Ok(LeaseRefreshStart::WaitForInFlight),
            Some(AuthLeasePhase::Released) | None => Ok(LeaseRefreshStart::Started(
                LeaseRefreshLifecycle::InitialAcquire,
            )),
        }
    }

    pub(crate) fn complete_refresh(
        &self,
        authorizer_label: &str,
        lifecycle: LeaseRefreshLifecycle,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<u64, AuthError> {
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
        Ok(self.handle.snapshot(&self.lease_key).generation)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
enum LeaseRefreshStart {
    Started(LeaseRefreshLifecycle),
    WaitForInFlight,
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) fn token_is_fresh_at(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    expires_at - now > Duration::seconds(AUTH_LEASE_TTL_REFRESH_WINDOW_SECS as i64)
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
fn lease_epoch_secs_is_fresh_at(expires_at: u64, now: DateTime<Utc>) -> bool {
    let expires_at = i64::try_from(expires_at).unwrap_or(i64::MAX);
    expires_at.saturating_sub(now.timestamp()) > AUTH_LEASE_TTL_REFRESH_WINDOW_SECS as i64
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) fn oauth_endpoint_failure_is_permanent(status: u16, body: &str) -> bool {
    if endpoint_failure_is_transient(status, body) {
        return false;
    }

    if matches!(status, 401 | 403) {
        return true;
    }

    matches!(status, 400) && body_mentions_permanent_oauth_error(body)
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) fn endpoint_failure_is_transient(status: u16, body: &str) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500..=599)
        || body_mentions_any(
            body,
            &[
                "temporarily_unavailable",
                "temporary_unavailable",
                "server_error",
                "rate_limit",
                "rate_limited",
                "too_many_requests",
                "timeout",
                "timed out",
                "try again",
            ],
        )
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
fn body_mentions_permanent_oauth_error(body: &str) -> bool {
    body_mentions_any(
        body,
        &[
            "invalid_client",
            "invalid_grant",
            "unauthorized_client",
            "invalid_scope",
            "access_denied",
            "permission_denied",
        ],
    )
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
fn body_mentions_any(body: &str, needles: &[&str]) -> bool {
    let body = body.to_ascii_lowercase();
    needles.iter().any(|needle| body.contains(needle))
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
