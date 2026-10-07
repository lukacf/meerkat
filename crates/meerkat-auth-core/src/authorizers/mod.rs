//! Dynamic `HttpAuthorizer` implementations for cloud backends:
//! AWS (SigV4 for Bedrock), Google (ADC + metadata), Azure AD
//! (client-credentials OAuth2).
//!
//! Each authorizer acquires and caches a credential/token and adds the
//! appropriate `Authorization` (and service-specific) headers on every
//! call to [`meerkat_core::HttpAuthorizer::authorize`].

use std::sync::Arc;

#[cfg(any(feature = "azure-ad", feature = "gcp-auth", feature = "aws-sigv4"))]
use chrono::{DateTime, Utc};
#[cfg(any(feature = "azure-ad", feature = "gcp-auth", feature = "aws-sigv4"))]
use meerkat_core::AuthError;
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use meerkat_core::handles::AuthLeaseSnapshot;
#[cfg(any(feature = "azure-ad", feature = "gcp-auth", feature = "aws-sigv4"))]
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, CredentialUseDisposition, CredentialUseIntent,
    DslTransitionError, GeneratedAuthLeaseHandle, LeaseKey,
};
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use meerkat_core::{AuthLoginLifecycleGuard, RefreshFailureObservation};

/// Shared closure type for env-variable lookup. Used by authorizers that
/// want to remain hermetic in tests by taking a closure rather than
/// reading `std::env::var` directly. The process-env implementation is
/// `Arc::new(|k| std::env::var(k).ok())`.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Clone)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth", feature = "aws-sigv4"))]
pub(crate) struct LeaseFreshnessObserver {
    handle: GeneratedAuthLeaseHandle,
    lease_key: LeaseKey,
}

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
const AUTH_LEASE_REFRESH_WAIT_POLL_MS: u64 = 10;
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
const AUTH_LEASE_REFRESH_WAIT_TIMEOUT_SECS: u64 = 30;

#[cfg(any(feature = "azure-ad", feature = "gcp-auth", feature = "aws-sigv4"))]
impl LeaseFreshnessObserver {
    pub(crate) fn new(handle: GeneratedAuthLeaseHandle, lease_key: LeaseKey) -> Self {
        Self { handle, lease_key }
    }

    /// Consult the per-binding AuthMachine for a credential-use verdict at
    /// `now` before signing with inline-resolved credential material (AWS
    /// SigV4), acquiring a `Valid` lease the first time the binding is absent.
    /// Unlike [`cached_token_is_fresh`](Self::cached_token_is_fresh) there is no
    /// endpoint-fetched token to cache-coherence-check: SigV4 resolves the
    /// credential inline per request, so the lease — not an implicit
    /// always-fresh assumption — owns whether the credential is still usable.
    ///
    /// `expires_at` carries the credential's bound expiry when one is known
    /// (STS session tokens); `None` models Env/Static credentials with no
    /// expiry as an explicit `Valid` (no-expiry) lease phase via a far-future
    /// sentinel rather than implicit always-fresh.
    ///
    /// The AuthMachine owns the `(lifecycle_phase, credential_present, intent)`
    /// -> disposition policy. `Authorized` -> proceed to sign; `LeaseAbsent` ->
    /// acquire a `Valid` lease and proceed (first use); `ReauthRequired` ->
    /// `Err(UserReauthRequired)`; every refresh disposition (expired/expiring
    /// STS credential) -> `Err(RefreshRequired)` so the signer fails closed
    /// instead of signing with stale material.
    #[cfg(feature = "aws-sigv4")]
    pub(crate) fn ensure_valid_for_signing(
        &self,
        authorizer_label: &str,
        now: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(), AuthError> {
        if let Some(expires_at) = expires_at
            && expires_at <= now
        {
            return Err(AuthError::Expired);
        }
        self.handle
            .observe_credential_freshness(
                &self.lease_key,
                epoch_secs(now),
                AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .map_err(|err| self.observer_error(authorizer_label, "observe_freshness", err))?;
        let disposition = self
            .handle
            .resolve_credential_use_admission(&self.lease_key, CredentialUseIntent::UseCredential)
            .map_err(|err| {
                self.observer_error(authorizer_label, "resolve_credential_use_admission", err)
            })?;
        match disposition {
            CredentialUseDisposition::Authorized => Ok(()),
            CredentialUseDisposition::LeaseAbsent => {
                // First use of this binding: acquire a `Valid` lease so the
                // AuthMachine — not the shell — owns the credential's validity.
                // No-expiry Env/Static creds acquire with a far-future sentinel
                // (`Valid`, no-expiry); STS creds acquire with their bound
                // expiry so a later observation can move them to Expired.
                let acquire_expiry = expires_at.map(epoch_secs).unwrap_or(u64::MAX);
                self.handle
                    .acquire_lease(&self.lease_key, acquire_expiry)
                    .map_err(|err| self.observer_error(authorizer_label, "acquire_lease", err))?;
                Ok(())
            }
            CredentialUseDisposition::ReauthRequired => Err(AuthError::UserReauthRequired),
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::AlreadyRefreshing => Err(AuthError::RefreshRequired),
        }
    }

    pub(crate) fn expires_at(&self) -> Option<DateTime<Utc>> {
        let snapshot = self.handle.snapshot(&self.lease_key);
        snapshot
            .expires_at
            .and_then(|secs| i64::try_from(secs).ok())
            .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
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

/// Refresh-lifecycle methods used by the endpoint-fetched-token authorizers
/// (Google ADC, Azure AD). The AWS SigV4 authorizer signs with credential
/// material resolved inline per request and never fetches/caches a token from
/// a refresh endpoint, so it consults
/// [`LeaseFreshnessObserver::ensure_valid_for_signing`] only and does not
/// compile this block.
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
impl LeaseFreshnessObserver {
    pub(crate) async fn cached_token_is_fresh(
        &self,
        authorizer_label: &str,
        expires_at: DateTime<Utc>,
        lease_generation: Option<u64>,
        now: impl FnOnce() -> DateTime<Utc> + Send,
    ) -> Result<bool, AuthError> {
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&self.lease_key).await;
        // Sample only after waiting for custody so the machine sees current time.
        let now = now();
        self.handle
            .observe_credential_freshness(
                &self.lease_key,
                epoch_secs(now),
                AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .map_err(|err| self.observer_error(authorizer_label, "observe_freshness", err))?;
        // The credential-use disposition is owned by the per-binding AuthMachine:
        // we feed only the typed `UseCredential` intent and mirror the verdict.
        // No handwritten `match snapshot.phase` usability fork lives here.
        // Authorized -> the lease is fresh-and-usable, proceed to the pure
        // cache-coherence equality checks below; RefreshRequired/AlreadyRefreshing/
        // LeaseAbsent -> not usable, refresh (Ok(false)); ReauthRequired ->
        // interactive reauth error. This preserves the prior `Valid -> proceed;
        // ReauthRequired -> Err; else -> Ok(false)` behavior exactly: a cached
        // token only reaches this gate after Acquire/CompleteRefresh published a
        // credential, so the live phase is `Valid` iff the lease is fresh and
        // `credential_present`.
        let disposition = self
            .handle
            .resolve_credential_use_admission(&self.lease_key, CredentialUseIntent::UseCredential)
            .map_err(|err| {
                self.observer_error(authorizer_label, "resolve_credential_use_admission", err)
            })?;
        match disposition {
            CredentialUseDisposition::Authorized => {}
            CredentialUseDisposition::ReauthRequired => {
                return Err(AuthError::UserReauthRequired);
            }
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::AlreadyRefreshing
            | CredentialUseDisposition::LeaseAbsent => return Ok(false),
        }
        let snapshot = self.handle.snapshot(&self.lease_key);

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
        // above, and AuthMachine classified the credential-use admission as
        // `Authorized` for this `now`/window (otherwise the disposition would be
        // RefreshRequired/AlreadyRefreshing/LeaseAbsent and we would have
        // returned `Ok(false)` at the admission match). The remaining checks
        // here are cache-coherence (does the cached token belong to the current
        // lease generation and expiry truth?), NOT a freshness re-derivation.
        // Once they pass, the machine's `Authorized` verdict IS the freshness
        // answer — the shell must not recompute it with its own window
        // comparison.
        Ok(true)
    }

    pub(crate) async fn begin_refresh(
        &self,
        authorizer_label: &str,
        mode: LeaseRefreshMode,
    ) -> Result<LeaseRefreshPreparation, AuthError> {
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(AUTH_LEASE_REFRESH_WAIT_TIMEOUT_SECS);
        loop {
            match self.try_begin_refresh(authorizer_label, mode).await? {
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

    async fn try_begin_refresh(
        &self,
        authorizer_label: &str,
        mode: LeaseRefreshMode,
    ) -> Result<LeaseRefreshStart, AuthError> {
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&self.lease_key).await;
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
                Ok(LeaseRefreshStart::Started(LeaseRefreshPreparation {
                    lifecycle: LeaseRefreshLifecycle::Refresh,
                    started: self.handle.snapshot(&self.lease_key),
                }))
            }
            CredentialUseDisposition::ReauthRequired => Err(AuthError::UserReauthRequired),
            // `RefreshDisallowed` is only emitted by the OAuth-login disposition,
            // not the `BeginRefresh` intent; fail closed onto a refresh-required
            // error if it ever surfaces here.
            CredentialUseDisposition::RefreshDisallowed => Err(AuthError::RefreshRequired),
            CredentialUseDisposition::AlreadyRefreshing => Ok(LeaseRefreshStart::WaitForInFlight),
            CredentialUseDisposition::LeaseAbsent => match mode {
                LeaseRefreshMode::AcquireOrRefresh => {
                    Ok(LeaseRefreshStart::Started(LeaseRefreshPreparation {
                        lifecycle: LeaseRefreshLifecycle::InitialAcquire,
                        started: self.handle.snapshot(&self.lease_key),
                    }))
                }
                LeaseRefreshMode::ExistingCredential => Err(AuthError::RefreshRequired),
            },
        }
    }

    /// Publish only the exact started predecessor. The returned existing lease
    /// guard keeps the derived cache installation in the same short mutation
    /// boundary. No caller may retain this guard across token HTTP.
    pub(crate) async fn complete_refresh(
        &self,
        authorizer_label: &str,
        preparation: LeaseRefreshPreparation,
        expires_at: DateTime<Utc>,
        now: impl FnOnce() -> DateTime<Utc> + Send,
    ) -> Result<(u64, AuthLoginLifecycleGuard), AuthError> {
        let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&self.lease_key).await;
        if self.handle.snapshot(&self.lease_key) != preparation.started {
            return Err(AuthError::StaleCredential);
        }
        // A token response may expire while this completion waits for custody.
        let now = now();
        let expires_at = epoch_secs(expires_at);
        let transition = match preparation.lifecycle {
            LeaseRefreshLifecycle::InitialAcquire => self
                .handle
                .acquire_lease(&self.lease_key, expires_at)
                .map_err(|err| self.observer_error(authorizer_label, "acquire_lease", err))?,
            LeaseRefreshLifecycle::Refresh => {
                match self
                    .handle
                    .complete_refresh(&self.lease_key, expires_at, epoch_secs(now))
                {
                    Ok(transition) => transition,
                    Err(err) => {
                        // An error alone does not prove that an owner transition
                        // was atomic. Close only our unchanged, guard-rejected
                        // refresh; never settle an advanced or replaced owner.
                        if self.handle.snapshot(&self.lease_key) != preparation.started {
                            return Err(AuthError::StaleCredential);
                        }
                        if err.is_guard_rejected() {
                            // A response that expired during custody does not
                            // establish permanent credential failure. Let the
                            // generated classifier close this failed attempt.
                            self.handle
                                .refresh_failed(
                                    &self.lease_key,
                                    RefreshFailureObservation::transient(),
                                )
                                .map_err(|err| {
                                    self.observer_error(authorizer_label, "refresh_failed", err)
                                })?;
                        }
                        return Err(self.observer_error(authorizer_label, "complete_refresh", err));
                    }
                }
            }
        };
        Ok((transition.generation(), guard))
    }

    pub(crate) async fn refresh_failed(
        &self,
        authorizer_label: &str,
        preparation: LeaseRefreshPreparation,
        observation: RefreshFailureObservation,
    ) -> Result<(), AuthError> {
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&self.lease_key).await;
        if self.handle.snapshot(&self.lease_key) != preparation.started {
            return Err(AuthError::StaleCredential);
        }
        if preparation.lifecycle == LeaseRefreshLifecycle::Refresh {
            self.handle
                .refresh_failed(&self.lease_key, observation)
                .map_err(|err| self.observer_error(authorizer_label, "refresh_failed", err))?;
        }
        Ok(())
    }
}

/// Initial acquisition belongs to ordinary authorization. Request-free native
/// maintenance can refresh only an existing generated credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) enum LeaseRefreshMode {
    AcquireOrRefresh,
    ExistingCredential,
}

#[derive(Debug, PartialEq, Eq)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) struct LeaseRefreshPreparation {
    lifecycle: LeaseRefreshLifecycle,
    started: AuthLeaseSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) enum LeaseRefreshLifecycle {
    InitialAcquire,
    Refresh,
}

#[derive(Debug, PartialEq, Eq)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
enum LeaseRefreshStart {
    Started(LeaseRefreshPreparation),
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

#[cfg(any(feature = "azure-ad", feature = "gcp-auth", feature = "aws-sigv4"))]
fn epoch_secs(ts: DateTime<Utc>) -> u64 {
    ts.timestamp().max(0) as u64
}

#[cfg(test)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::connection::{BindingId, RealmId};
    use meerkat_core::handles::{AuthLeaseHandle, AuthLeasePhase, GeneratedAuthLeaseHandle};

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

    #[tokio::test]
    async fn initial_acquire_returns_generation_from_accepted_transition() {
        let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let lease_key = lease_key();
        let observer = LeaseFreshnessObserver::new(
            generated_auth_lease_handle_for_test(Arc::clone(&handle)),
            lease_key,
        );
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let now = DateTime::<Utc>::from_timestamp(1_799_999_000, 0).unwrap();

        let preparation = observer
            .begin_refresh("race-test", LeaseRefreshMode::AcquireOrRefresh)
            .await
            .unwrap();
        let (generation, _guard) = observer
            .complete_refresh("race-test", preparation, expires_at, || now)
            .await
            .unwrap();

        assert_eq!(generation, 1);
    }

    #[tokio::test]
    async fn refresh_returns_generation_from_accepted_transition() {
        let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let lease_key = lease_key();
        handle.acquire_lease(&lease_key, 1_799_999_500).unwrap();
        let observer = LeaseFreshnessObserver::new(
            generated_auth_lease_handle_for_test(Arc::clone(&handle)),
            lease_key,
        );
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let now = DateTime::<Utc>::from_timestamp(1_799_999_000, 0).unwrap();

        let preparation = observer
            .begin_refresh("race-test", LeaseRefreshMode::ExistingCredential)
            .await
            .unwrap();
        let (generation, _guard) = observer
            .complete_refresh("race-test", preparation, expires_at, || now)
            .await
            .unwrap();

        assert_eq!(generation, 2);
    }

    #[tokio::test]
    async fn cache_clock_is_sampled_after_lifecycle_custody() {
        use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
        use std::task::Poll;

        let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let key = LeaseKey::new(
            RealmId::parse("dev").unwrap(),
            BindingId::parse("cloud-cache-clock").unwrap(),
            None,
        );
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let transition = handle.acquire_lease(&key, epoch_secs(expires_at)).unwrap();
        let observer = LeaseFreshnessObserver::new(
            generated_auth_lease_handle_for_test(Arc::clone(&handle)),
            key.clone(),
        );
        let clock = AtomicI64::new(expires_at.timestamp() - 1_000);
        let read_clock =
            || DateTime::<Utc>::from_timestamp(clock.load(Ordering::SeqCst), 0).unwrap();
        assert!(
            observer
                .cached_token_is_fresh(
                    "clock-control",
                    expires_at,
                    Some(transition.generation()),
                    read_clock,
                )
                .await
                .unwrap(),
            "the same actual owner is usable before the custody wait"
        );

        let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&key).await;
        let samples = AtomicUsize::new(0);
        let mut check = Box::pin(observer.cached_token_is_fresh(
            "clock-contention",
            expires_at,
            Some(transition.generation()),
            || {
                samples.fetch_add(1, Ordering::SeqCst);
                read_clock()
            },
        ));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(check.as_mut(), cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(samples.load(Ordering::SeqCst), 0);
        assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Valid));
        clock.store(expires_at.timestamp() + 1, Ordering::SeqCst);
        drop(guard);

        assert!(
            !check.await.unwrap(),
            "a custody wait cannot retain an old freshness verdict"
        );
        assert_eq!(samples.load(Ordering::SeqCst), 1);
        assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Expired));
        assert_eq!(
            handle
                .resolve_credential_use_admission(&key, CredentialUseIntent::HoldAuthority)
                .unwrap(),
            CredentialUseDisposition::RefreshRequired
        );
    }

    #[tokio::test]
    async fn completion_clock_is_sampled_after_lifecycle_custody() {
        use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
        use std::task::Poll;

        for cross_expiry in [false, true] {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = LeaseKey::new(
                RealmId::parse("dev").unwrap(),
                BindingId::parse(if cross_expiry {
                    "cloud-completion-clock-expired"
                } else {
                    "cloud-completion-clock-current"
                })
                .unwrap(),
                None,
            );
            handle.acquire_lease(&key, u64::MAX).unwrap();
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key.clone(),
            );
            let preparation = observer
                .begin_refresh("completion-clock", LeaseRefreshMode::ExistingCredential)
                .await
                .unwrap();
            let started = handle.snapshot(&key);
            assert_eq!(started.phase, Some(AuthLeasePhase::Refreshing));
            let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
            let clock = AtomicI64::new(expires_at.timestamp() - 1_000);
            let samples = AtomicUsize::new(0);
            let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&key).await;
            let mut completion = Box::pin(observer.complete_refresh(
                "completion-clock",
                preparation,
                expires_at,
                || {
                    samples.fetch_add(1, Ordering::SeqCst);
                    DateTime::<Utc>::from_timestamp(clock.load(Ordering::SeqCst), 0).unwrap()
                },
            ));
            std::future::poll_fn(|cx| {
                assert!(std::future::Future::poll(completion.as_mut(), cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert_eq!(samples.load(Ordering::SeqCst), 0);
            assert_eq!(handle.snapshot(&key), started);
            if cross_expiry {
                clock.store(expires_at.timestamp() + 1, Ordering::SeqCst);
            }
            drop(guard);

            let result = completion.await;
            assert_eq!(samples.load(Ordering::SeqCst), 1);
            if cross_expiry {
                assert!(
                    matches!(result, Err(AuthError::Other(_))),
                    "the actual generated CompleteRefresh must reject the now-expired response"
                );
                let closed = handle.snapshot(&key);
                assert_eq!(closed.phase, Some(AuthLeasePhase::Expiring));
                assert_eq!(closed.generation, started.generation);
                assert_eq!(closed.expires_at, started.expires_at);
                assert_eq!(closed.credential_present, started.credential_present);
                assert_eq!(
                    closed.credential_published_at_millis,
                    started.credential_published_at_millis
                );
                let next = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    observer.begin_refresh(
                        "completion-follow-up",
                        LeaseRefreshMode::ExistingCredential,
                    ),
                )
                .await
                .expect("the failed HTTP owner must not strand the next request")
                .unwrap();
                let healthy_expiry = expires_at + chrono::Duration::hours(1);
                let (generation, _guard) = observer
                    .complete_refresh("completion-follow-up", next, healthy_expiry, || {
                        DateTime::<Utc>::from_timestamp(clock.load(Ordering::SeqCst), 0).unwrap()
                    })
                    .await
                    .unwrap();
                assert_eq!(generation, started.generation + 1);
                assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Valid));
                assert_eq!(
                    handle.snapshot(&key).expires_at,
                    Some(epoch_secs(healthy_expiry))
                );
            } else {
                let (generation, _guard) = result.unwrap();
                assert_eq!(generation, started.generation + 1);
                assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Valid));
                assert_eq!(
                    handle.snapshot(&key).expires_at,
                    Some(epoch_secs(expires_at))
                );
            }
        }
    }

    #[tokio::test]
    async fn expired_completion_never_closes_a_replaced_owner() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::Poll;

        for successor in ["released", "valid", "refreshing"] {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = LeaseKey::new(
                RealmId::parse("dev").unwrap(),
                BindingId::parse(format!("cloud-expired-replaced-{successor}")).unwrap(),
                None,
            );
            handle.acquire_lease(&key, u64::MAX).unwrap();
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key.clone(),
            );
            let preparation = observer
                .begin_refresh("expired-replaced", LeaseRefreshMode::ExistingCredential)
                .await
                .unwrap();
            let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
            let samples = AtomicUsize::new(0);
            let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&key).await;
            let mut completion = Box::pin(observer.complete_refresh(
                "expired-replaced",
                preparation,
                expires_at,
                || {
                    samples.fetch_add(1, Ordering::SeqCst);
                    expires_at + chrono::Duration::seconds(1)
                },
            ));
            std::future::poll_fn(|cx| {
                assert!(std::future::Future::poll(completion.as_mut(), cx).is_pending());
                Poll::Ready(())
            })
            .await;
            handle.release_lease_with_guard(&key, &guard).unwrap();
            if successor != "released" {
                handle.acquire_lease(&key, u64::MAX).unwrap();
            }
            if successor == "refreshing" {
                handle.begin_refresh(&key).unwrap();
            }
            let replacement = handle.snapshot(&key);
            drop(guard);

            assert!(matches!(completion.await, Err(AuthError::StaleCredential)));
            assert_eq!(samples.load(Ordering::SeqCst), 0);
            assert_eq!(handle.snapshot(&key), replacement, "{successor}");
        }
    }

    /// FOLD 1: `try_begin_refresh` mirrors the AuthMachine's machine-routed
    /// `ResolveCredentialUseAdmission { intent: BeginRefresh }` disposition for
    /// every reachable phase. No handwritten `phase -> disposition` fork lives
    /// in the observer; the per-binding AuthMachine owns the verdict and the
    /// observer only mirrors it onto `LeaseRefreshStart` / the reauth error.
    #[tokio::test]
    async fn try_begin_refresh_mirrors_authmachine_disposition_for_every_phase() {
        // No registered lease (None phase) -> machine reports LeaseAbsent ->
        // InitialAcquire.
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                lease_key(),
            );
            assert!(
                matches!(
                    observer
                        .try_begin_refresh("absent", LeaseRefreshMode::AcquireOrRefresh)
                        .await
                        .unwrap(),
                    LeaseRefreshStart::Started(LeaseRefreshPreparation {
                        lifecycle: LeaseRefreshLifecycle::InitialAcquire,
                        ..
                    })
                ),
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
            assert!(
                matches!(
                    observer
                        .try_begin_refresh("valid", LeaseRefreshMode::AcquireOrRefresh)
                        .await
                        .unwrap(),
                    LeaseRefreshStart::Started(LeaseRefreshPreparation {
                        lifecycle: LeaseRefreshLifecycle::Refresh,
                        ..
                    })
                ),
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
            assert!(matches!(
                observer
                    .try_begin_refresh("expiring", LeaseRefreshMode::AcquireOrRefresh)
                    .await
                    .unwrap(),
                LeaseRefreshStart::Started(LeaseRefreshPreparation {
                    lifecycle: LeaseRefreshLifecycle::Refresh,
                    ..
                })
            ),);
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
            assert!(matches!(
                observer
                    .try_begin_refresh("expired", LeaseRefreshMode::AcquireOrRefresh)
                    .await
                    .unwrap(),
                LeaseRefreshStart::Started(LeaseRefreshPreparation {
                    lifecycle: LeaseRefreshLifecycle::Refresh,
                    ..
                })
            ),);
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
                observer
                    .try_begin_refresh("refreshing", LeaseRefreshMode::AcquireOrRefresh)
                    .await
                    .unwrap(),
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
                    observer
                        .try_begin_refresh("reauth", LeaseRefreshMode::AcquireOrRefresh)
                        .await,
                    Err(AuthError::UserReauthRequired)
                ),
                "reauth-required lease must surface UserReauthRequired via the machine's ReauthRequired disposition"
            );
        }
    }

    /// FOLD A: `cached_token_is_fresh` mirrors the AuthMachine's machine-routed
    /// `ResolveCredentialUseAdmission { intent: UseCredential }` disposition for
    /// every reachable phase. No handwritten `match snapshot.phase` usability
    /// fork lives in the observer; the per-binding AuthMachine owns the verdict
    /// and the observer only mirrors it onto Ok(true) (proceed to coherence) /
    /// Ok(false) (refresh) / Err(UserReauthRequired).
    #[tokio::test]
    async fn cached_token_is_fresh_mirrors_authmachine_disposition_for_every_phase() {
        let far_future = DateTime::<Utc>::from_timestamp(2_000_000_000, 0).unwrap();
        let now = DateTime::<Utc>::from_timestamp(1_000_000_000, 0).unwrap();

        // Valid + credential present + coherent cache (matching generation +
        // expiry) -> Authorized -> Ok(true).
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            let transition = handle.acquire_lease(&key, epoch_secs(far_future)).unwrap();
            assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Valid));
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key,
            );
            assert!(
                observer
                    .cached_token_is_fresh(
                        "valid",
                        far_future,
                        Some(transition.generation()),
                        || now
                    )
                    .await
                    .unwrap(),
                "valid+coherent lease must be fresh via the machine's Authorized disposition"
            );
        }

        // Expiring + credential present -> RefreshRequired -> Ok(false).
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            let transition = handle.acquire_lease(&key, epoch_secs(far_future)).unwrap();
            handle.mark_expiring(&key).unwrap();
            assert_eq!(handle.snapshot(&key).phase, Some(AuthLeasePhase::Expiring));
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key,
            );
            assert!(
                !observer
                    .cached_token_is_fresh(
                        "expiring",
                        far_future,
                        Some(transition.generation()),
                        || now,
                    )
                    .await
                    .unwrap(),
                "expiring lease must refresh via the machine's RefreshRequired disposition"
            );
        }

        // Expired + credential present -> RefreshRequired -> Ok(false). A
        // near-past expiry plus a future freshness observation drives Expired.
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            let near_past = DateTime::<Utc>::from_timestamp(1_000, 0).unwrap();
            let transition = handle.acquire_lease(&key, epoch_secs(near_past)).unwrap();
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key,
            );
            assert!(
                !observer
                    .cached_token_is_fresh(
                        "expired",
                        near_past,
                        Some(transition.generation()),
                        || now,
                    )
                    .await
                    .unwrap(),
                "expired lease must refresh via the machine's RefreshRequired disposition"
            );
        }

        // Refreshing + credential present -> RefreshRequired -> Ok(false).
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            let transition = handle.acquire_lease(&key, epoch_secs(far_future)).unwrap();
            handle.begin_refresh(&key).unwrap();
            assert_eq!(
                handle.snapshot(&key).phase,
                Some(AuthLeasePhase::Refreshing)
            );
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key,
            );
            assert!(
                !observer
                    .cached_token_is_fresh(
                        "refreshing",
                        far_future,
                        Some(transition.generation()),
                        || now,
                    )
                    .await
                    .unwrap(),
                "refreshing lease must refresh via the machine's RefreshRequired disposition"
            );
        }

        // ReauthRequired -> Err(UserReauthRequired).
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            let transition = handle.acquire_lease(&key, epoch_secs(far_future)).unwrap();
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
                    observer
                        .cached_token_is_fresh(
                            "reauth",
                            far_future,
                            Some(transition.generation()),
                            || now,
                        )
                        .await,
                    Err(AuthError::UserReauthRequired)
                ),
                "reauth-required lease must surface UserReauthRequired via the machine's ReauthRequired disposition"
            );
        }

        // Released / absent binding -> LeaseAbsent -> Ok(false). `release_lease`
        // removes the binding entirely, so its snapshot phase is `None`; either
        // way the machine classifies it LeaseAbsent and the observer refreshes.
        {
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let key = lease_key();
            handle.acquire_lease(&key, epoch_secs(far_future)).unwrap();
            handle.release_lease(&key).unwrap();
            assert!(
                matches!(
                    handle.snapshot(&key).phase,
                    None | Some(AuthLeasePhase::Released)
                ),
                "released lease must be absent or Released, never live"
            );
            let observer = LeaseFreshnessObserver::new(
                generated_auth_lease_handle_for_test(Arc::clone(&handle)),
                key,
            );
            assert!(
                !observer
                    .cached_token_is_fresh("released", far_future, Some(1), || now)
                    .await
                    .unwrap(),
                "released/absent lease must refresh via the machine's LeaseAbsent disposition"
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
