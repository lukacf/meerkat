//! Auth lifecycle publication helpers.
//!
//! TokenStore owns credential material. AuthMachine owns lifecycle state.
//! Surfaces that write or clear credentials use these helpers so the public
//! status path observes the machine-owned lease instead of deriving phase from
//! persisted token bytes.

use chrono::{DateTime, Utc};
use thiserror::Error;

#[cfg(not(target_arch = "wasm32"))]
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, Weak},
};

use super::status::AuthStatusPhase;
use super::token_store::{
    PersistedAuthMode, PersistedTokens, TokenKey, TokenStore, TokenStoreError,
};
use crate::connection::AuthBindingRef;
use crate::generated::auth_lease_durable_lifecycle_marker as durable_marker;
use crate::handles::{
    AuthLeasePhase, AuthLeaseRestoreSnapshot, AuthLeaseSnapshot, AuthLeaseTransition,
    DslTransitionError, GeneratedAuthLeaseHandle, LeaseKey,
};

#[cfg(not(target_arch = "wasm32"))]
type LoginLifecycleLockMap = parking_lot::Mutex<HashMap<LeaseKey, Weak<tokio::sync::Mutex<()>>>>;

#[cfg(not(target_arch = "wasm32"))]
static LOGIN_LIFECYCLE_LOCKS: OnceLock<LoginLifecycleLockMap> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn login_lifecycle_locks() -> &'static LoginLifecycleLockMap {
    LOGIN_LIFECYCLE_LOCKS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

/// Process-local guard that serializes credential commit, terminal OAuth
/// consume, and compensating rollback for one auth binding.
#[cfg(not(target_arch = "wasm32"))]
pub struct AuthLoginLifecycleGuard {
    _lease_key: LeaseKey,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn acquire_auth_login_lifecycle_guard(lease_key: &LeaseKey) -> AuthLoginLifecycleGuard {
    let lock = {
        let mut locks = login_lifecycle_locks().lock();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(lease_key).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(tokio::sync::Mutex::new(()));
            locks.insert(lease_key.clone(), Arc::downgrade(&lock));
            lock
        }
    };
    AuthLoginLifecycleGuard {
        _lease_key: lease_key.clone(),
        _guard: lock.lock_owned().await,
    }
}

pub fn persisted_token_expires_at_epoch_secs(tokens: &PersistedTokens) -> u64 {
    tokens
        .expires_at
        .map(|ts| ts.timestamp().max(0) as u64)
        .unwrap_or(u64::MAX)
}

pub fn persisted_auth_mode_uses_oauth_login_lifecycle(mode: PersistedAuthMode) -> bool {
    matches!(
        mode,
        PersistedAuthMode::ChatgptOauth
            | PersistedAuthMode::ExternalTokens
            | PersistedAuthMode::ClaudeAiOauth
            | PersistedAuthMode::OauthToApiKey
            | PersistedAuthMode::GoogleOauth
    )
}

/// Whether a credential of this mode can be created directly by writing a
/// secret at the `auth/profile/create` surfaces (RPC/REST). Direct-secret
/// modes (api key / static bearer) are creatable; OAuth-login-lifecycle modes
/// must use the `auth/login/start` + `auth/login/complete` flow instead.
///
/// Single owner of the "direct-secret only at create surfaces" predicate over
/// the typed [`PersistedAuthMode`].
pub fn persisted_auth_mode_is_directly_creatable(mode: PersistedAuthMode) -> bool {
    !persisted_auth_mode_uses_oauth_login_lifecycle(mode)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenLifecyclePublication {
    pub phase: Option<AuthLeasePhase>,
    pub generation: Option<u64>,
    pub expires_at: u64,
    pub credential_published_at_millis: Option<u64>,
}

#[derive(Debug, Error)]
pub enum TokenLifecycleMarkerError {
    #[error(
        "AuthMachine lifecycle transition belongs to `{transition_key}`, not token key `{token_key}`"
    )]
    LeaseKeyMismatch {
        token_key: String,
        transition_key: String,
    },
    #[error(
        "AuthMachine lifecycle transition expires_at {transition_expires_at} does not match token expires_at {token_expires_at}"
    )]
    ExpiresAtMismatch {
        token_expires_at: u64,
        transition_expires_at: u64,
    },
    #[error("AuthMachine lifecycle transition did not carry a credential publication time")]
    CredentialPublicationTimeMissing,
}

fn mark_tokens_lifecycle_published_inner(
    key: &TokenKey,
    tokens: &PersistedTokens,
    phase: AuthLeasePhase,
    generation: u64,
    credential_published_at_millis: u64,
) -> PersistedTokens {
    let mut marked = tokens.clone();
    marked.metadata = durable_marker::metadata_with_marker(
        &marked.metadata,
        durable_marker::DurableAuthLifecycleMarker {
            token_key: key.clone(),
            phase,
            expires_at: persisted_token_expires_at_epoch_secs(tokens),
            generation,
            credential_published_at_millis,
        },
    );
    marked
}

#[cfg(test)]
fn mark_tokens_lifecycle_published(key: &TokenKey, tokens: &PersistedTokens) -> PersistedTokens {
    mark_tokens_lifecycle_published_inner(key, tokens, AuthLeasePhase::Valid, 1, 1)
}

pub fn mark_tokens_lifecycle_published_for_transition(
    key: &TokenKey,
    tokens: &PersistedTokens,
    transition: &AuthLeaseTransition,
) -> Result<PersistedTokens, TokenLifecycleMarkerError> {
    let token_lease_key =
        LeaseKey::new(key.realm.clone(), key.binding.clone(), key.profile.clone());
    if transition.lease_key() != &token_lease_key {
        return Err(TokenLifecycleMarkerError::LeaseKeyMismatch {
            token_key: token_lease_key.to_string(),
            transition_key: transition.lease_key().to_string(),
        });
    }
    let token_expires_at = persisted_token_expires_at_epoch_secs(tokens);
    if transition.expires_at() != token_expires_at {
        return Err(TokenLifecycleMarkerError::ExpiresAtMismatch {
            token_expires_at,
            transition_expires_at: transition.expires_at(),
        });
    }
    let credential_published_at_millis = transition
        .credential_published_at_millis()
        .ok_or(TokenLifecycleMarkerError::CredentialPublicationTimeMissing)?;
    Ok(mark_tokens_lifecycle_published_inner(
        key,
        tokens,
        transition.phase(),
        transition.generation(),
        credential_published_at_millis,
    ))
}

pub fn tokens_lifecycle_published(tokens: &PersistedTokens) -> bool {
    durable_marker::metadata_has_valid_marker(&tokens.metadata)
}

pub fn tokens_lifecycle_published_generation(tokens: &PersistedTokens) -> Option<u64> {
    tokens_lifecycle_publication(tokens).and_then(|publication| publication.generation)
}

pub fn tokens_lifecycle_publication(tokens: &PersistedTokens) -> Option<TokenLifecyclePublication> {
    tokens_lifecycle_publication_inner(tokens, false)
}

pub fn tokens_lifecycle_publication_with_explicit_expiry(
    tokens: &PersistedTokens,
) -> Option<TokenLifecyclePublication> {
    tokens_lifecycle_publication_inner(tokens, true)
}

fn tokens_lifecycle_publication_inner(
    tokens: &PersistedTokens,
    _require_explicit_expiry: bool,
) -> Option<TokenLifecyclePublication> {
    let marker = durable_marker::read_marker_from_metadata(&tokens.metadata)?;
    Some(TokenLifecyclePublication {
        phase: Some(marker.phase),
        generation: Some(marker.generation),
        expires_at: marker.expires_at,
        credential_published_at_millis: Some(marker.credential_published_at_millis),
    })
}

pub fn tokens_lifecycle_published_credential_time(tokens: &PersistedTokens) -> Option<u64> {
    durable_marker::read_marker_from_metadata(&tokens.metadata)
        .map(|marker| marker.credential_published_at_millis)
}

/// Record a credential acquisition in the AuthMachine lease (an in-memory DSL
/// transition; no durable I/O).
///
/// Acquire-first ordering contract (shared by the MCP OAuth login and the
/// REST/CLI/RPC token-commit surfaces): call this BEFORE any durable
/// `TokenStore::save`, stamp the durable lifecycle marker from the returned
/// transition via [`mark_tokens_lifecycle_published_for_transition`], and
/// persist the credential in that single marked write. The marker is the
/// durable proof-of-acquisition; an unmarked persisted token is dead data and
/// is rejected by `restore_marked_token_lifecycle` /
/// [`rehydrate_marked_tokens_for_status`] and the resolver admission gate. A
/// rejected acquisition mutates nothing, so the caller propagates the error
/// without compensation; if the durable save fails AFTER acquisition, the
/// caller must roll the acquired lease back so no half-state survives.
pub fn publish_token_lifecycle_acquired(
    handle: &GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
) -> Result<AuthLeaseTransition, DslTransitionError> {
    let lease_key = LeaseKey::from_auth_binding(auth_binding);
    handle.acquire_lease(&lease_key, persisted_token_expires_at_epoch_secs(tokens))
}

pub fn publish_token_lifecycle_released(
    handle: &GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
) -> Result<(), DslTransitionError> {
    let lease_key = LeaseKey::from_auth_binding(auth_binding);
    handle.release_lease(&lease_key)
}

#[derive(Debug, Error)]
pub enum TokenLifecycleClearError {
    #[error("AuthMachine lifecycle release failed: {0}")]
    AuthMachineRelease(DslTransitionError),
    #[error("TokenStore clear failed: {0}")]
    TokenStoreClear(TokenStoreError),
    /// The durable clear failed AND rolling the staged lease release back to
    /// the pre-clear snapshot was rejected. Both typed faults are carried;
    /// durable truth still holds the credential, so the next status rehydrate
    /// restores the lease projection from the durable lifecycle marker.
    #[error("TokenStore clear failed ({clear}); staged lease release rollback failed ({restore})")]
    StagedReleaseRestore {
        clear: TokenStoreError,
        restore: DslTransitionError,
    },
}

/// Clear persisted token material and release the AuthMachine lifecycle.
///
/// The lease release is STAGED before the durable clear commits, so no lease
/// operation ever runs after the durable credential is gone — the
/// "resurrected live lease over a cleared credential" interleaving is
/// unrepresentable:
///
/// - If staging the release fails (a release-observer fault — which aborts
///   the release before its authoritative transition — or a machine
///   rejection), the typed
///   [`TokenLifecycleClearError::AuthMachineRelease`] fault propagates and
///   the durable clear NEVER runs. Durable truth retains the credential and
///   the lease projection still matches it; the clear is retryable.
/// - If the durable clear commit fails, the staged release is rolled back
///   from the pre-stage snapshot (legal: the durable record still holds the
///   credential, so lease truth re-aligns with durable truth) and the typed
///   [`TokenLifecycleClearError::TokenStoreClear`] fault propagates.
/// - Once the durable clear has committed, the operation is complete: the
///   lease was already released at staging, and nothing fallible follows the
///   commit.
pub async fn clear_tokens_and_publish_lifecycle_released(
    store: &dyn TokenStore,
    handle: &GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
) -> Result<(), TokenLifecycleClearError> {
    let key = TokenKey::from_auth_binding(auth_binding);
    let lease_key = LeaseKey::from_auth_binding(auth_binding);

    // Stage: release the lease BEFORE the durable commit, capturing the
    // pre-stage snapshot for rollback if the commit fails.
    let staged = handle.capture_auth_lifecycle_restore_snapshot(&lease_key);
    publish_token_lifecycle_released(handle, auth_binding)
        .map_err(TokenLifecycleClearError::AuthMachineRelease)?;

    // Commit: one durable mutation destroys the credential and its lifecycle
    // marker together.
    if let Err(clear_err) = store.clear(&key).await {
        // Pre-commit rollback: durable truth still holds the credential, so
        // restoring the staged release keeps the lease projection aligned.
        if let Err(restore_err) = restore_token_lifecycle_snapshot(handle, &staged) {
            return Err(TokenLifecycleClearError::StagedReleaseRestore {
                clear: clear_err,
                restore: restore_err,
            });
        }
        return Err(TokenLifecycleClearError::TokenStoreClear(clear_err));
    }
    Ok(())
}

/// Restore an AuthMachine lease projection from a previously captured
/// snapshot.
///
/// Callers that need to compensate a token write after a later step fails can
/// use this with the token snapshot captured before the write. If the previous
/// snapshot had no credential-backed active phase, this helper is a no-op; it
/// must not recreate credential authority from token bytes alone.
pub fn restore_token_lifecycle_snapshot(
    handle: &GeneratedAuthLeaseHandle,
    captured: &AuthLeaseRestoreSnapshot,
) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
    let snapshot = captured.snapshot();
    if !snapshot.credential_present {
        return Ok(None);
    }
    let Some(phase) = snapshot.phase else {
        return Ok(None);
    };
    if phase == AuthLeasePhase::Released {
        return Ok(None);
    }

    handle.restore_auth_lifecycle_snapshot(captured)
}

pub fn lease_snapshot_expires_at_datetime(snapshot: &AuthLeaseSnapshot) -> Option<DateTime<Utc>> {
    snapshot
        .expires_at
        .and_then(|secs| i64::try_from(secs).ok())
        .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
}

#[derive(Debug)]
pub struct PublishedAuthStatus<'a> {
    pub phase: AuthStatusPhase,
    pub expires_at: Option<DateTime<Utc>>,
    pub tokens: Option<&'a PersistedTokens>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Error)]
pub enum AuthStatusRehydrateError {
    #[error("token store error: {0}")]
    TokenStore(#[from] TokenStoreError),
    #[error("AuthMachine lifecycle acquire failed: {0}")]
    LifecycleAcquire(DslTransitionError),
    #[error("AuthMachine lifecycle restore failed: {0}")]
    LifecycleRestore(DslTransitionError),
    #[error("AuthMachine lifecycle rollback failed after token marker save failure: {0}")]
    LifecycleRollback(DslTransitionError),
    #[error("TokenStore lifecycle marker save failed: {0}")]
    MarkerSave(TokenStoreError),
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn restore_marked_token_lifecycle(
    auth_lease: &GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
) -> Result<Option<PersistedTokens>, AuthStatusRehydrateError> {
    let key = TokenKey::from_auth_binding(auth_binding);
    let Some(publication) =
        durable_marker::restore_publication_from_metadata(&tokens.metadata, &key)
    else {
        return Ok(None);
    };
    if publication.expires_at() != persisted_token_expires_at_epoch_secs(tokens) {
        return Ok(None);
    }
    let lease_key = LeaseKey::from_auth_binding(auth_binding);
    auth_lease
        .restore_published_credential_lifecycle(&lease_key, &publication)
        .map_err(AuthStatusRehydrateError::LifecycleRestore)?;
    Ok(Some(tokens.clone()))
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn rehydrate_marked_tokens_for_status(
    token_store: &dyn TokenStore,
    auth_lease: &GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    expected_mode: PersistedAuthMode,
    now: DateTime<Utc>,
) -> Result<Option<PersistedTokens>, AuthStatusRehydrateError> {
    let key = TokenKey::from_auth_binding(auth_binding);
    let Some(tokens) = token_store.load(&key).await? else {
        // Durable truth holds no credential for this binding. The in-process
        // lease is a projection of durable truth: a credential-bearing lease
        // that outlived its durable record (e.g. a clear whose release
        // transition was rejected) is released here so public status cannot
        // keep projecting a credential the store no longer holds.
        let lease_key = LeaseKey::from_auth_binding(auth_binding);
        let captured = auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key);
        let snapshot = captured.snapshot();
        let lease_is_live = snapshot.credential_present
            && snapshot
                .phase
                .is_some_and(|phase| phase != AuthLeasePhase::Released);
        if lease_is_live {
            auth_lease
                .release_lease(&lease_key)
                .map_err(AuthStatusRehydrateError::LifecycleRestore)?;
        }
        return Ok(None);
    };
    if tokens.auth_mode != expected_mode {
        return Ok(None);
    }
    let restored = restore_marked_token_lifecycle(auth_lease, auth_binding, &tokens)?;
    if restored.is_some() {
        let lease_key = LeaseKey::from_auth_binding(auth_binding);
        auth_lease
            .observe_credential_freshness(
                &lease_key,
                now.timestamp().max(0) as u64,
                crate::handles::AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .map_err(AuthStatusRehydrateError::LifecycleRestore)?;
    }
    Ok(restored)
}

pub fn project_published_auth_status<'a>(
    now: DateTime<Utc>,
    stored: Option<&'a PersistedTokens>,
    snapshot: &AuthLeaseSnapshot,
) -> PublishedAuthStatus<'a> {
    let phase = AuthStatusPhase::from_lease_snapshot(now, snapshot);
    if phase.is_no_live_lease() {
        return PublishedAuthStatus {
            phase,
            expires_at: None,
            tokens: None,
        };
    }
    PublishedAuthStatus {
        phase,
        expires_at: lease_snapshot_expires_at_datetime(snapshot)
            .or_else(|| stored.and_then(|tokens| tokens.expires_at)),
        tokens: stored,
    }
}

pub fn oauth_status_projection_snapshot_from_newer_marker(
    snapshot: &AuthLeaseSnapshot,
    tokens: &PersistedTokens,
) -> Option<AuthLeaseSnapshot> {
    let _ = (snapshot, tokens);
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    use crate::auth::PersistedAuthMode;
    use crate::handles::AuthLeasePhase;

    fn oauth_tokens_with_metadata(metadata: serde_json::Value) -> PersistedTokens {
        PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("access".into()),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata,
        }
    }

    fn marker_test_key() -> TokenKey {
        TokenKey::parse("dev", "default_openai").unwrap()
    }

    #[test]
    fn direct_secret_modes_are_creatable_oauth_login_modes_are_not() {
        // Direct-secret persisted modes are creatable at the auth/profile/create
        // surfaces; OAuth-login-lifecycle modes are not.
        for mode in [PersistedAuthMode::ApiKey, PersistedAuthMode::StaticBearer] {
            assert!(
                persisted_auth_mode_is_directly_creatable(mode),
                "{mode:?} must be directly creatable"
            );
        }
        for mode in [
            PersistedAuthMode::ChatgptOauth,
            PersistedAuthMode::ExternalTokens,
            PersistedAuthMode::ClaudeAiOauth,
            PersistedAuthMode::OauthToApiKey,
            PersistedAuthMode::GoogleOauth,
        ] {
            assert!(
                !persisted_auth_mode_is_directly_creatable(mode),
                "{mode:?} (oauth-login lifecycle) must NOT be directly creatable"
            );
        }
    }

    #[test]
    fn lifecycle_marker_is_added_to_managed_store_tokens() {
        let key = marker_test_key();
        let api_key = PersistedTokens::api_key("sk-test");
        let marked_api_key = mark_tokens_lifecycle_published(&key, &api_key);
        assert!(tokens_lifecycle_published(&marked_api_key));

        let oauth = oauth_tokens_with_metadata(serde_json::Value::Null);
        let marked = mark_tokens_lifecycle_published(&key, &oauth);
        assert!(tokens_lifecycle_published(&marked));
        assert!(!tokens_lifecycle_published(&oauth));
    }

    #[test]
    fn lifecycle_marker_preserves_existing_object_metadata() {
        let key = marker_test_key();
        let oauth = oauth_tokens_with_metadata(serde_json::json!({
            "provider": "openai",
        }));

        let marked = mark_tokens_lifecycle_published(&key, &oauth);

        assert!(tokens_lifecycle_published(&marked));
        assert_eq!(marked.metadata["provider"], "openai");
    }

    #[test]
    fn unmarked_orphan_tokens_are_dead_data_for_lifecycle_restore() {
        // Vault property: a persisted token WITHOUT the durable lifecycle
        // marker (the on-disk shape an interrupted save-then-acquire commit
        // used to leave behind) carries no proof-of-acquisition. Every
        // restore/rehydration entry point must treat it as dead data.
        let key = marker_test_key();
        let orphan = oauth_tokens_with_metadata(serde_json::Value::Null);

        assert!(!tokens_lifecycle_published(&orphan));
        assert!(tokens_lifecycle_publication(&orphan).is_none());
        assert!(
            durable_marker::restore_publication_from_metadata(&orphan.metadata, &key).is_none()
        );

        // Arbitrary non-marker metadata is equally dead.
        let decorated = oauth_tokens_with_metadata(serde_json::json!({
            "provider": "openai",
            "source": "legacy-import",
        }));
        assert!(!tokens_lifecycle_published(&decorated));
        assert!(
            durable_marker::restore_publication_from_metadata(&decorated.metadata, &key).is_none()
        );
    }

    #[test]
    fn lifecycle_marker_restore_is_bound_to_token_identity() {
        let key = marker_test_key();
        let wrong_key = TokenKey::parse("dev", "other_openai").unwrap();
        let oauth = oauth_tokens_with_metadata(serde_json::Value::Null);
        let marked = mark_tokens_lifecycle_published(&key, &oauth);

        assert!(
            durable_marker::restore_publication_from_metadata(&marked.metadata, &key).is_some()
        );
        assert!(
            durable_marker::restore_publication_from_metadata(&marked.metadata, &wrong_key)
                .is_none()
        );
    }

    #[test]
    fn lifecycle_marker_relation_is_bound_to_token_identity() {
        let key = marker_test_key();
        let wrong_key = TokenKey::parse("dev", "other_openai").unwrap();
        let oauth = oauth_tokens_with_metadata(serde_json::Value::Null);
        let marked = mark_tokens_lifecycle_published(&key, &oauth);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(u64::MAX),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(1),
        };

        assert!(durable_marker::marker_payload_valid_for_tokens(
            &marked, &key
        ));
        assert!(!durable_marker::marker_payload_valid_for_tokens(
            &marked, &wrong_key
        ));
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(&marked, &snapshot, &key),
            durable_marker::AuthLeaseDurableMarkerRelation::Matches
        );
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(&marked, &snapshot, &wrong_key),
            durable_marker::AuthLeaseDurableMarkerRelation::Invalid
        );
    }

    #[test]
    fn lifecycle_marker_rejects_wrong_schema_version() {
        let oauth = oauth_tokens_with_metadata(serde_json::json!({
            "meerkat_auth_lifecycle": {
                "published": true,
                "version": 2,
                "authority": "auth_machine",
                "protocol": "auth_lease_lifecycle_publication",
                "phase": "valid",
                "generation": 1,
                "expires_at": u64::MAX,
                "credential_published_at_millis": 1,
            }
        }));

        assert!(!tokens_lifecycle_published(&oauth));
        assert!(tokens_lifecycle_publication(&oauth).is_none());
    }

    #[test]
    fn published_status_projects_lease_phase_without_token_material() {
        let now = Utc::now();
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some((now + chrono::Duration::hours(1)).timestamp() as u64),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: None,
        };

        let status = project_published_auth_status(now, None, &snapshot);

        assert_eq!(status.phase, AuthStatusPhase::Valid);
        assert!(status.expires_at.is_some());
        assert!(status.tokens.is_none());
    }
}
