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
use meerkat_core::AuthError;
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use meerkat_core::RefreshFailureObservation;
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeasePhase, CredentialUseDisposition,
    CredentialUseIntent, DslTransitionError, GeneratedAuthLeaseHandle, LeaseKey,
};

/// Shared closure type for env-variable lookup. Used by authorizers that
/// want to remain hermetic in tests by taking a closure rather than
/// reading `std::env::var` directly. The process-env implementation is
/// `Arc::new(|k| std::env::var(k).ok())`.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Clone)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) struct LeaseFreshnessObserver {
    handle: GeneratedAuthLeaseHandle,
    lease_key: LeaseKey,
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
const AUTH_LEASE_REFRESH_WAIT_POLL_MS: u64 = 10;
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
const AUTH_LEASE_REFRESH_WAIT_TIMEOUT_SECS: u64 = 30;

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
impl LeaseFreshnessObserver {
    pub(crate) fn new(handle: GeneratedAuthLeaseHandle, lease_key: LeaseKey) -> Self {
        Self { handle, lease_key }
    }

    pub(crate) fn cached_token_is_fresh(
        &self,
        authorizer_label: &str,
        expires_at: DateTime<Utc>,
        lease_generation: Option<u64>,
        now: DateTime<Utc>,
    ) -> Result<bool, AuthError> {
        self.handle
            .observe_credential_freshness(
                &self.lease_key,
                epoch_secs(now),
                AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .map_err(|err| self.observer_error(authorizer_label, "observe_freshness", err))?;
        let snapshot = self.handle.snapshot(&self.lease_key);
        match snapshot.phase {
            Some(AuthLeasePhase::Valid) => {}
            Some(AuthLeasePhase::ReauthRequired) => {
                return Err(AuthError::UserReauthRequired);
            }
            Some(
                AuthLeasePhase::Expiring
                | AuthLeasePhase::Expired
                | AuthLeasePhase::Refreshing
                | AuthLeasePhase::Released,
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

        // Freshness is machine-owned. We already drove
        // `observe_credential_freshness(.., epoch_secs(now), AUTH_LEASE_TTL_REFRESH_WINDOW_SECS)`
        // above, and AuthMachine classified the lease as `Valid` for this
        // `now`/window (otherwise `snapshot.phase` would be Expiring/Expired/…
        // and we would have returned `Ok(false)` at the phase match). The
        // remaining checks here are cache-coherence (does the cached token
        // belong to the current lease generation and expiry truth?), NOT a
        // freshness re-derivation. Once they pass, the machine's `Valid`
        // verdict IS the freshness answer — the shell must not recompute it
        // with its own window comparison.
        Ok(true)
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
        let now = Utc::now();
        // Drive the machine's freshness classification first so the
        // credential-use admission below reads the up-to-date phase.
        self.handle
            .observe_credential_freshness(
                &self.lease_key,
                epoch_secs(now),
                AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .map_err(|err| self.observer_error(authorizer_label, "observe_freshness", err))?;
        // The begin-refresh disposition is owned by the per-binding AuthMachine:
        // we feed only the typed `BeginRefresh` intent and mirror the verdict.
        // No handwritten `phase -> disposition` fork lives here.
        let disposition = self
            .handle
            .resolve_credential_use_admission(&self.lease_key, CredentialUseIntent::BeginRefresh)
            .map_err(|err| {
                self.observer_error(authorizer_label, "resolve_credential_use_admission", err)
            })?;
        match disposition {
            // A live credential exists in valid/expiring/expired: begin the
            // refresh and report it started (preserving the prior `Valid`/
            // `Expiring`/`Expired` -> begin_refresh + Started(Refresh) path).
            // The machine never emits `Authorized` for the BeginRefresh intent;
            // we mirror it identically to `RefreshRequired` to fail closed onto
            // the refresh path the `Valid` case historically took.
            CredentialUseDisposition::RefreshRequired | CredentialUseDisposition::Authorized => {
                self.handle
                    .begin_refresh(&self.lease_key)
                    .map_err(|err| self.observer_error(authorizer_label, "begin_refresh", err))?;
                Ok(LeaseRefreshStart::Started(LeaseRefreshLifecycle::Refresh))
            }
            CredentialUseDisposition::ReauthRequired => Err(AuthError::UserReauthRequired),
            CredentialUseDisposition::AlreadyRefreshing => Ok(LeaseRefreshStart::WaitForInFlight),
            CredentialUseDisposition::LeaseAbsent => Ok(LeaseRefreshStart::Started(
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
        let transition = match lifecycle {
            LeaseRefreshLifecycle::InitialAcquire => self
                .handle
                .acquire_lease(&self.lease_key, expires_at)
                .map_err(|err| self.observer_error(authorizer_label, "acquire_lease", err))?,
            LeaseRefreshLifecycle::Refresh => self
                .handle
                .complete_refresh(&self.lease_key, expires_at, epoch_secs(now))
                .map_err(|err| self.observer_error(authorizer_label, "complete_refresh", err))?,
        };
        Ok(transition.generation())
    }

    pub(crate) fn refresh_failed(
        &self,
        authorizer_label: &str,
        lifecycle: LeaseRefreshLifecycle,
        observation: RefreshFailureObservation,
    ) -> Result<(), AuthError> {
        if lifecycle == LeaseRefreshLifecycle::Refresh {
            self.handle
                .refresh_failed(&self.lease_key, observation)
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
pub(crate) fn oauth_endpoint_failure_observation(
    status: u16,
    body: &str,
) -> RefreshFailureObservation {
    RefreshFailureObservation::oauth_token_endpoint(
        status,
        crate::auth_oauth::oauth_token_endpoint_error_code(body),
    )
}

#[cfg(feature = "gcp-auth")]
pub(crate) fn endpoint_failure_is_transient(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500..=599)
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
fn epoch_secs(ts: DateTime<Utc>) -> u64 {
    ts.timestamp().max(0) as u64
}

#[cfg(test)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::connection::{BindingId, RealmId};
    use meerkat_core::handles::{AuthLeaseHandle, GeneratedAuthLeaseHandle};

    fn generated_auth_lease_handle_for_test(
        handle: Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
    ) -> GeneratedAuthLeaseHandle {
        meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
            handle,
        )
        .expect("runtime AuthLeaseHandle is certified by generated AuthMachine authority")
    }

    fn lease_key() -> LeaseKey {
        LeaseKey::new(
            RealmId::parse("dev").unwrap(),
            BindingId::parse("cloud").unwrap(),
            None,
        )
    }

    #[test]
    fn initial_acquire_returns_generation_from_accepted_transition() {
        let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let lease_key = lease_key();
        let observer = LeaseFreshnessObserver::new(
            generated_auth_lease_handle_for_test(Arc::clone(&handle)),
            lease_key,
        );
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let now = DateTime::<Utc>::from_timestamp(1_799_999_000, 0).unwrap();

        let generation = observer
            .complete_refresh(
                "race-test",
                LeaseRefreshLifecycle::InitialAcquire,
                expires_at,
                now,
            )
            .unwrap();

        assert_eq!(generation, 1);
    }

    #[test]
    fn refresh_returns_generation_from_accepted_transition() {
        let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let lease_key = lease_key();
        handle.acquire_lease(&lease_key, 1_799_999_500).unwrap();
        handle.begin_refresh(&lease_key).unwrap();
        let observer = LeaseFreshnessObserver::new(
            generated_auth_lease_handle_for_test(Arc::clone(&handle)),
            lease_key,
        );
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let now = DateTime::<Utc>::from_timestamp(1_799_999_000, 0).unwrap();

        let generation = observer
            .complete_refresh("race-test", LeaseRefreshLifecycle::Refresh, expires_at, now)
            .unwrap();

        assert_eq!(generation, 2);
    }

    /// FOLD 1: `try_begin_refresh` mirrors the AuthMachine's machine-routed
    /// `ResolveCredentialUseAdmission { intent: BeginRefresh }` disposition for
    /// every reachable phase. No handwritten `phase -> disposition` fork lives
    /// in the observer; the per-binding AuthMachine owns the verdict and the
    /// observer only mirrors it onto `LeaseRefreshStart` / the reauth error.
    #[test]
    fn try_begin_refresh_mirrors_authmachine_disposition_for_every_phase() {
        // No registered lease (None phase) -> machine reports LeaseAbsent ->
        // InitialAcquire.
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                lease_key(),
            );
            assert_eq!(
                observer.try_begin_refresh("absent").unwrap(),
                LeaseRefreshStart::Started(LeaseRefreshLifecycle::InitialAcquire),
                "absent lease must InitialAcquire via the machine's LeaseAbsent disposition"
            );
        }

        // Valid + credential present -> RefreshRequired -> begin_refresh +
        // Started(Refresh). Far-future expiry keeps the lease Valid through the
        // freshness observation.
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            handle.acquire_lease(&key, u64::MAX).unwrap();
            assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Valid));
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key.clone(),
            );
            assert_eq!(
                observer.try_begin_refresh("valid").unwrap(),
                LeaseRefreshStart::Started(LeaseRefreshLifecycle::Refresh),
                "valid lease must begin refresh via the machine's RefreshRequired disposition"
            );
            assert_eq!(
                handle.snapshot(&key).phase,
                Some(AuthLeasePhase::Refreshing),
                "begin_refresh side effect must move the machine to Refreshing"
            );
        }

        // Expiring + credential present -> RefreshRequired -> Started(Refresh).
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            handle.acquire_lease(&key, u64::MAX).unwrap();
            handle.mark_expiring(&key).unwrap();
            assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Expiring));
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key,
            );
            assert_eq!(
                observer.try_begin_refresh("expiring").unwrap(),
                LeaseRefreshStart::Started(LeaseRefreshLifecycle::Refresh),
            );
        }

        // Expired + credential present -> RefreshRequired -> Started(Refresh).
        // A near-past expiry plus a freshness observation in the future drives
        // the machine into Expired.
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            handle.acquire_lease(&key, 1_000).unwrap();
            handle
                .observe_credential_freshness(&key, 1_000_000, AUTH_LEASE_TTL_REFRESH_WINDOW_SECS)
                .unwrap();
            assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Expired));
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key,
            );
            assert_eq!(
                observer.try_begin_refresh("expired").unwrap(),
                LeaseRefreshStart::Started(LeaseRefreshLifecycle::Refresh),
            );
        }

        // Refreshing -> AlreadyRefreshing -> WaitForInFlight (no double-begin).
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            handle.acquire_lease(&key, u64::MAX).unwrap();
            handle.begin_refresh(&key).unwrap();
            assert_eq!(
                handle.snapshot(&key).phase,
                Some(AuthLeasePhase::Refreshing)
            );
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key,
            );
            assert_eq!(
                observer.try_begin_refresh("refreshing").unwrap(),
                LeaseRefreshStart::WaitForInFlight,
                "in-flight refresh must wait via the machine's AlreadyRefreshing disposition"
            );
        }

        // ReauthRequired -> ReauthRequired -> Err(UserReauthRequired).
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            handle.acquire_lease(&key, u64::MAX).unwrap();
            handle.mark_reauth_required(&key).unwrap();
            assert_eq!(
                handle.snapshot(&key).phase,
                Some(AuthLeasePhase::ReauthRequired)
            );
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key,
            );
            assert!(
                matches!(
                    observer.try_begin_refresh("reauth"),
                    Err(AuthError::UserReauthRequired)
                ),
                "reauth-required lease must surface UserReauthRequired via the machine's ReauthRequired disposition"
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
