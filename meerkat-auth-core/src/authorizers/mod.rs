//! Dynamic `HttpAuthorizer` implementations for cloud backends:
//! AWS (SigV4 for Bedrock), Google (ADC + metadata), Azure AD
//! (client-credentials OAuth2).
//!
//! Cloud authorizers acquire credential material and add the appropriate
//! `Authorization` (and service-specific) headers on every call to
//! [`meerkat_core::HttpAuthorizer::authorize`]. Token-material reuse is gated
//! by AuthMachine lease truth rather than by private freshness checks.

use std::sync::Arc;

#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use chrono::{DateTime, Utc};
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use meerkat_core::AuthError;
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot,
    DslTransitionError, LeaseKey,
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
        lease_generation: u64,
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

        let Some(lease_expires_at) = snapshot.expires_at else {
            tracing::warn!(
                authorizer = %authorizer_label,
                lease_key = %self.lease_key,
                snapshot_generation = snapshot.generation,
                "cloud authorizer cache has no auth lease expiry truth; refreshing"
            );
            return Ok(false);
        };

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
                LeaseRefreshStart::Retry => continue,
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
        let snapshot = self.handle.snapshot(&self.lease_key);
        match snapshot.phase {
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => {
                match self
                    .handle
                    .begin_refresh_if_snapshot(&self.lease_key, &snapshot)
                {
                    Ok(Some(transition)) => Ok(LeaseRefreshStart::Started(
                        LeaseRefreshLifecycle::Refresh(AuthLeaseSnapshot {
                            phase: Some(AuthLeasePhase::Refreshing),
                            expires_at: snapshot.expires_at,
                            credential_present: snapshot.credential_present,
                            generation: transition.generation,
                            credential_published_at_millis: snapshot.credential_published_at_millis,
                        }),
                    )),
                    Ok(None) => match self.handle.snapshot(&self.lease_key).phase {
                        Some(AuthLeasePhase::Refreshing) => Ok(LeaseRefreshStart::WaitForInFlight),
                        Some(AuthLeasePhase::ReauthRequired) => Err(AuthError::Expired),
                        Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => {
                            Ok(LeaseRefreshStart::Retry)
                        }
                        Some(AuthLeasePhase::Released) | None => Ok(LeaseRefreshStart::Retry),
                    },
                    Err(err) => match self.handle.snapshot(&self.lease_key).phase {
                        Some(AuthLeasePhase::Refreshing) => Ok(LeaseRefreshStart::WaitForInFlight),
                        Some(AuthLeasePhase::ReauthRequired) => Err(AuthError::Expired),
                        _ => Err(self.observer_error(
                            authorizer_label,
                            "begin_refresh_if_snapshot",
                            err,
                        )),
                    },
                }
            }
            Some(AuthLeasePhase::ReauthRequired) => Err(AuthError::Expired),
            Some(AuthLeasePhase::Refreshing) => Ok(LeaseRefreshStart::WaitForInFlight),
            Some(AuthLeasePhase::Released) | None => Ok(LeaseRefreshStart::Started(
                LeaseRefreshLifecycle::InitialAcquire(snapshot),
            )),
        }
    }

    pub(crate) fn complete_refresh(
        &self,
        authorizer_label: &str,
        lifecycle: LeaseRefreshLifecycle,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<LeaseRefreshCompletion, AuthError> {
        let expires_at = epoch_secs(expires_at);
        let transition = match lifecycle {
            LeaseRefreshLifecycle::InitialAcquire(expected) => match self
                .handle
                .acquire_lease_if_snapshot(&self.lease_key, &expected, expires_at)
                .map_err(|err| {
                    self.observer_error(authorizer_label, "acquire_lease_if_snapshot", err)
                })? {
                Some(transition) => transition,
                None => {
                    return self.initial_acquire_lost_race_completion(authorizer_label, now);
                }
            },
            LeaseRefreshLifecycle::Refresh(expected) => self
                .handle
                .complete_refresh_if_snapshot(
                    &self.lease_key,
                    &expected,
                    expires_at,
                    epoch_secs(now),
                )
                .map_err(|err| {
                    self.observer_error(authorizer_label, "complete_refresh_if_snapshot", err)
                })?
                .ok_or_else(|| {
                    AuthError::Other(format!(
                        "{authorizer_label} auth lease refresh completion lost a race for {}; retry required",
                        self.lease_key
                    ))
                })?,
        };
        Ok(LeaseRefreshCompletion::Accepted(transition.generation))
    }

    fn initial_acquire_lost_race_completion(
        &self,
        authorizer_label: &str,
        now: DateTime<Utc>,
    ) -> Result<LeaseRefreshCompletion, AuthError> {
        let snapshot = self.handle.snapshot(&self.lease_key);
        match snapshot.phase {
            Some(AuthLeasePhase::Valid) => {
                if let Some(expires_at) = snapshot.expires_at
                    && lease_epoch_secs_is_fresh_at(expires_at, now)
                {
                    tracing::debug!(
                        authorizer = %authorizer_label,
                        lease_key = %self.lease_key,
                        snapshot_generation = snapshot.generation,
                        snapshot_expires_at = expires_at,
                        "cloud authorizer initial acquire lost a race but observed fresh AuthMachine lease truth"
                    );
                    return Ok(LeaseRefreshCompletion::Retry);
                }
            }
            Some(AuthLeasePhase::ReauthRequired) => return Err(AuthError::Expired),
            Some(
                AuthLeasePhase::Expiring | AuthLeasePhase::Refreshing | AuthLeasePhase::Released,
            )
            | None => {}
        }
        Err(AuthError::Other(format!(
            "{authorizer_label} auth lease initial acquire lost a race for {}; retry required",
            self.lease_key
        )))
    }

    pub(crate) fn refresh_failed(
        &self,
        authorizer_label: &str,
        lifecycle: LeaseRefreshLifecycle,
        permanent: bool,
    ) -> Result<(), AuthError> {
        if let LeaseRefreshLifecycle::Refresh(expected) = lifecycle {
            self.handle
                .refresh_failed_if_snapshot(&self.lease_key, &expected, permanent)
                .map_err(|err| {
                    self.observer_error(authorizer_label, "refresh_failed_if_snapshot", err)
                })?;
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

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) enum LeaseRefreshLifecycle {
    InitialAcquire(AuthLeaseSnapshot),
    Refresh(AuthLeaseSnapshot),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
pub(crate) enum LeaseRefreshCompletion {
    Accepted(u64),
    Retry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
enum LeaseRefreshStart {
    Started(LeaseRefreshLifecycle),
    WaitForInFlight,
    Retry,
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

#[cfg(test)]
#[cfg(any(feature = "azure-ad", feature = "gcp-auth"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::connection::{BindingId, RealmId};
    use meerkat_core::handles::{AuthLeaseSnapshot, AuthLeaseTransition};
    use std::sync::Mutex;

    struct SnapshotRaceAuthLeaseHandle {
        snapshot: Mutex<AuthLeaseSnapshot>,
        generation: Mutex<u64>,
        accepted_generations: Mutex<Vec<u64>>,
    }

    struct BeginRefreshRaceAuthLeaseHandle {
        snapshot: Mutex<AuthLeaseSnapshot>,
    }

    struct BeginRefreshStaleValidRaceAuthLeaseHandle {
        snapshot: Mutex<AuthLeaseSnapshot>,
        unconditional_begin_calls: Mutex<u64>,
        conditional_begin_calls: Mutex<u64>,
    }

    impl Default for SnapshotRaceAuthLeaseHandle {
        fn default() -> Self {
            Self {
                snapshot: Mutex::new(AuthLeaseSnapshot {
                    phase: None,
                    expires_at: None,
                    credential_present: false,
                    generation: 0,
                    credential_published_at_millis: None,
                }),
                generation: Mutex::new(0),
                accepted_generations: Mutex::new(Vec::new()),
            }
        }
    }

    impl SnapshotRaceAuthLeaseHandle {
        fn accept_valid_transition(
            &self,
            expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            let accepted_generation = {
                let mut generation = self.generation.lock().unwrap();
                *generation += 1;
                *generation
            };
            self.accepted_generations
                .lock()
                .unwrap()
                .push(accepted_generation);
            *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(expires_at),
                credential_present: true,
                generation: accepted_generation + 1,
                credential_published_at_millis: None,
            };
            Ok(AuthLeaseTransition {
                generation: accepted_generation,
                credential_published_at_millis: None,
            })
        }

        fn accepted_generations(&self) -> Vec<u64> {
            self.accepted_generations.lock().unwrap().clone()
        }
    }

    impl AuthLeaseHandle for SnapshotRaceAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            self.accept_valid_transition(expires_at)
        }

        fn acquire_lease_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
            expires_at: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            if self.snapshot(lease_key) != *expected {
                return Ok(None);
            }
            self.acquire_lease(lease_key, expires_at).map(Some)
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn begin_refresh(
            &self,
            _lease_key: &LeaseKey,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            Ok(AuthLeaseTransition::new(1, Some(1)))
        }

        fn begin_refresh_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            if self.snapshot(lease_key) != *expected
                || !matches!(
                    expected.phase,
                    Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
                )
            {
                return Ok(None);
            }
            self.begin_refresh(lease_key).map(Some)
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            self.accept_valid_transition(new_expires_at)
        }

        fn complete_refresh_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
            new_expires_at: u64,
            now: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            if self.snapshot(lease_key) != *expected
                || expected.phase != Some(AuthLeasePhase::Refreshing)
            {
                return Ok(None);
            }
            self.complete_refresh(lease_key, new_expires_at, now)
                .map(Some)
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn refresh_failed_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _permanent: bool,
        ) -> Result<bool, DslTransitionError> {
            Ok(false)
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn mark_reauth_required_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
        ) -> Result<bool, DslTransitionError> {
            if self.snapshot(lease_key) != *expected
                || !matches!(
                    expected.phase,
                    Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
                )
            {
                return Ok(false);
            }
            self.mark_reauth_required(lease_key)?;
            Ok(true)
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshot.lock().unwrap().clone()
        }
    }

    impl Default for BeginRefreshRaceAuthLeaseHandle {
        fn default() -> Self {
            Self {
                snapshot: Mutex::new(AuthLeaseSnapshot {
                    phase: Some(AuthLeasePhase::Valid),
                    expires_at: Some(1_800_000_000),
                    credential_present: true,
                    generation: 7,
                    credential_published_at_millis: Some(7),
                }),
            }
        }
    }

    impl AuthLeaseHandle for BeginRefreshRaceAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            _expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            unreachable!("begin-refresh race test must not acquire")
        }

        fn acquire_lease_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _expires_at: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            unreachable!("begin-refresh race test must not acquire")
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            unreachable!("begin-refresh race test must not mark expiring")
        }

        fn begin_refresh(
            &self,
            _lease_key: &LeaseKey,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Refreshing),
                expires_at: Some(1_800_000_000),
                credential_present: true,
                generation: 8,
                credential_published_at_millis: Some(7),
            };
            Err(DslTransitionError::no_matching(
                "begin_refresh",
                "another actor moved the lease into refreshing",
            ))
        }

        fn begin_refresh_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            if self.snapshot(lease_key) != *expected
                || !matches!(
                    expected.phase,
                    Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
                )
            {
                return Ok(None);
            }
            self.begin_refresh(lease_key).map(Some)
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            unreachable!("begin-refresh race test must not complete refresh")
        }

        fn complete_refresh_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            unreachable!("begin-refresh race test must not complete refresh")
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            unreachable!("begin-refresh race test must not fail refresh")
        }

        fn refresh_failed_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _permanent: bool,
        ) -> Result<bool, DslTransitionError> {
            unreachable!("begin-refresh race test must not fail refresh")
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            unreachable!("begin-refresh race test must not require reauth")
        }

        fn mark_reauth_required_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
        ) -> Result<bool, DslTransitionError> {
            unreachable!("begin-refresh race test must not require reauth")
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            unreachable!("begin-refresh race test must not release")
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshot.lock().unwrap().clone()
        }
    }

    impl Default for BeginRefreshStaleValidRaceAuthLeaseHandle {
        fn default() -> Self {
            Self {
                snapshot: Mutex::new(AuthLeaseSnapshot {
                    phase: Some(AuthLeasePhase::Valid),
                    expires_at: Some(1_800_000_000),
                    credential_present: true,
                    generation: 7,
                    credential_published_at_millis: Some(7),
                }),
                unconditional_begin_calls: Mutex::new(0),
                conditional_begin_calls: Mutex::new(0),
            }
        }
    }

    impl BeginRefreshStaleValidRaceAuthLeaseHandle {
        fn unconditional_begin_calls(&self) -> u64 {
            *self.unconditional_begin_calls.lock().unwrap()
        }
    }

    impl AuthLeaseHandle for BeginRefreshStaleValidRaceAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            _expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not acquire")
        }

        fn acquire_lease_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _expires_at: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not acquire")
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not mark expiring")
        }

        fn begin_refresh(
            &self,
            _lease_key: &LeaseKey,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            *self.unconditional_begin_calls.lock().unwrap() += 1;
            *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Refreshing),
                expires_at: Some(1_800_000_000),
                credential_present: true,
                generation: 8,
                credential_published_at_millis: Some(7),
            };
            Ok(AuthLeaseTransition::new(8, Some(7)))
        }

        fn begin_refresh_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            let mut calls = self.conditional_begin_calls.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
                    phase: Some(AuthLeasePhase::Valid),
                    expires_at: Some(1_900_000_000),
                    credential_present: true,
                    generation: 8,
                    credential_published_at_millis: Some(8),
                };
                return Ok(None);
            }
            if self.snapshot(lease_key) != *expected {
                return Ok(None);
            }
            *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Refreshing),
                expires_at: expected.expires_at,
                credential_present: expected.credential_present,
                generation: expected.generation + 1,
                credential_published_at_millis: expected.credential_published_at_millis,
            };
            Ok(Some(AuthLeaseTransition::new(
                expected.generation + 1,
                expected.credential_published_at_millis,
            )))
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not complete refresh")
        }

        fn complete_refresh_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not complete refresh")
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not fail refresh")
        }

        fn refresh_failed_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _permanent: bool,
        ) -> Result<bool, DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not fail refresh")
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not require reauth")
        }

        fn mark_reauth_required_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
        ) -> Result<bool, DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not require reauth")
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            unreachable!("begin-refresh stale-valid race test must not release")
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshot.lock().unwrap().clone()
        }
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
        let handle = Arc::new(SnapshotRaceAuthLeaseHandle::default());
        let lease_key = lease_key();
        let expected = handle.snapshot(&lease_key);
        let observer = LeaseFreshnessObserver::new(handle.clone(), lease_key);
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let now = DateTime::<Utc>::from_timestamp(1_799_999_000, 0).unwrap();

        let completion = observer
            .complete_refresh(
                "race-test",
                LeaseRefreshLifecycle::InitialAcquire(expected),
                expires_at,
                now,
            )
            .unwrap();

        assert_eq!(handle.accepted_generations(), vec![1]);
        assert_eq!(completion, LeaseRefreshCompletion::Accepted(1));
    }

    #[test]
    fn initial_acquire_does_not_overwrite_newer_lease_truth() {
        let handle = Arc::new(SnapshotRaceAuthLeaseHandle::default());
        let lease_key = lease_key();
        let observer = LeaseFreshnessObserver::new(handle.clone(), lease_key.clone());
        let start = observer.try_begin_refresh("race-test").unwrap();
        let lifecycle = match start {
            LeaseRefreshStart::Started(LeaseRefreshLifecycle::InitialAcquire(snapshot)) => {
                LeaseRefreshLifecycle::InitialAcquire(snapshot)
            }
            other => panic!("expected initial acquire lifecycle, got {other:?}"),
        };
        handle.accept_valid_transition(1_700_000_000).unwrap();
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let now = DateTime::<Utc>::from_timestamp(1_799_999_000, 0).unwrap();

        let err = observer
            .complete_refresh("race-test", lifecycle, expires_at, now)
            .expect_err("conditional initial acquire must reject stale snapshot");

        assert!(matches!(err, AuthError::Other(_)), "got {err:?}");
        assert_eq!(
            handle.accepted_generations(),
            vec![1],
            "stale initial acquire must not publish a second generation"
        );
        assert_eq!(
            handle.snapshot(&lease_key).expires_at,
            Some(1_700_000_000),
            "newer AuthMachine lease truth must remain intact"
        );
    }

    #[test]
    fn initial_acquire_lost_race_discards_unaccepted_token_and_retries() {
        let handle = Arc::new(SnapshotRaceAuthLeaseHandle::default());
        let lease_key = lease_key();
        let observer = LeaseFreshnessObserver::new(handle.clone(), lease_key.clone());
        let start = observer.try_begin_refresh("race-test").unwrap();
        let lifecycle = match start {
            LeaseRefreshStart::Started(LeaseRefreshLifecycle::InitialAcquire(snapshot)) => {
                LeaseRefreshLifecycle::InitialAcquire(snapshot)
            }
            other => panic!("expected initial acquire lifecycle, got {other:?}"),
        };
        handle.accept_valid_transition(1_900_000_000).unwrap();
        let expires_at = DateTime::<Utc>::from_timestamp(1_950_000_000, 0).unwrap();
        let now = DateTime::<Utc>::from_timestamp(1_799_999_000, 0).unwrap();

        let completion = observer
            .complete_refresh("race-test", lifecycle, expires_at, now)
            .expect("fresh lease truth published by the winner should send the loser through AuthMachine again");

        assert_eq!(
            completion,
            LeaseRefreshCompletion::Retry,
            "loser must discard token material that AuthMachine did not accept"
        );
        assert_eq!(
            handle.accepted_generations(),
            vec![1],
            "stale initial acquire must not publish a second generation"
        );
        assert_eq!(
            handle.snapshot(&lease_key).expires_at,
            Some(1_900_000_000),
            "winner's AuthMachine lease truth must remain intact"
        );
    }

    #[test]
    fn refresh_returns_generation_from_accepted_transition() {
        let handle = Arc::new(SnapshotRaceAuthLeaseHandle::default());
        let lease_key = lease_key();
        let expected = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Refreshing),
            expires_at: Some(1_700_000_000),
            credential_present: true,
            generation: 3,
            credential_published_at_millis: Some(3),
        };
        *handle.snapshot.lock().unwrap() = expected.clone();
        let observer = LeaseFreshnessObserver::new(handle.clone(), lease_key);
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let now = DateTime::<Utc>::from_timestamp(1_799_999_000, 0).unwrap();

        let completion = observer
            .complete_refresh(
                "race-test",
                LeaseRefreshLifecycle::Refresh(expected),
                expires_at,
                now,
            )
            .unwrap();

        assert_eq!(handle.accepted_generations(), vec![1]);
        assert_eq!(completion, LeaseRefreshCompletion::Accepted(1));
    }

    #[test]
    fn begin_refresh_race_waits_for_inflight_refresh() {
        let handle = Arc::new(BeginRefreshRaceAuthLeaseHandle::default());
        let lease_key = lease_key();
        let observer = LeaseFreshnessObserver::new(handle, lease_key);

        let start = observer.try_begin_refresh("race-test").unwrap();

        assert_eq!(start, LeaseRefreshStart::WaitForInFlight);
    }

    #[tokio::test]
    async fn begin_refresh_stale_valid_race_retries_without_unconditional_owner_claim() {
        let handle = Arc::new(BeginRefreshStaleValidRaceAuthLeaseHandle::default());
        let lease_key = lease_key();
        let observer = LeaseFreshnessObserver::new(handle.clone(), lease_key.clone());

        let lifecycle = observer.begin_refresh("race-test").await.unwrap();

        let LeaseRefreshLifecycle::Refresh(refresh_snapshot) = lifecycle else {
            panic!("expected refresh lifecycle, got {lifecycle:?}");
        };
        assert_eq!(refresh_snapshot.expires_at, Some(1_900_000_000));
        assert_eq!(
            handle.unconditional_begin_calls(),
            0,
            "stale observer must not claim refresh ownership with unconditional begin_refresh"
        );
        assert_eq!(
            handle.snapshot(&lease_key).expires_at,
            Some(1_900_000_000),
            "refresh ownership must be based on the newer AuthMachine lease truth"
        );
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
