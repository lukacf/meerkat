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
use crate::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseRestoreSnapshot, AuthLeaseSnapshot,
    AuthLeaseTransition, DslTransitionError, LeaseKey,
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

const TOKEN_LIFECYCLE_METADATA_KEY: &str = "meerkat_auth_lifecycle";
const TOKEN_LIFECYCLE_PREVIOUS_METADATA_KEY: &str = "meerkat_previous_metadata";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenLifecyclePublication {
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
}

fn mark_tokens_lifecycle_published_inner(
    tokens: &PersistedTokens,
    generation: Option<u64>,
    credential_published_at_millis: Option<u64>,
) -> PersistedTokens {
    if !persisted_auth_mode_uses_oauth_login_lifecycle(tokens.auth_mode) {
        return tokens.clone();
    }

    let mut marked = tokens.clone();
    let mut marker = serde_json::json!({
        "published": true,
        "version": 2,
        "expires_at": persisted_token_expires_at_epoch_secs(tokens),
    });
    if let Some(generation) = generation
        && let Some(marker) = marker.as_object_mut()
    {
        marker.insert("generation".to_string(), serde_json::json!(generation));
    }
    if let Some(credential_published_at_millis) = credential_published_at_millis
        && let Some(marker) = marker.as_object_mut()
    {
        marker.insert(
            "credential_published_at_millis".to_string(),
            serde_json::json!(credential_published_at_millis),
        );
    }
    match &mut marked.metadata {
        serde_json::Value::Object(map) => {
            map.insert(TOKEN_LIFECYCLE_METADATA_KEY.to_string(), marker);
        }
        serde_json::Value::Null => {
            let mut metadata = serde_json::Map::new();
            metadata.insert(TOKEN_LIFECYCLE_METADATA_KEY.to_string(), marker);
            marked.metadata = serde_json::Value::Object(metadata);
        }
        _ => {
            let previous = std::mem::replace(&mut marked.metadata, serde_json::Value::Null);
            let mut metadata = serde_json::Map::new();
            metadata.insert(TOKEN_LIFECYCLE_METADATA_KEY.to_string(), marker);
            metadata.insert(TOKEN_LIFECYCLE_PREVIOUS_METADATA_KEY.to_string(), previous);
            marked.metadata = serde_json::Value::Object(metadata);
        }
    }
    marked
}

#[cfg(test)]
fn mark_tokens_lifecycle_published(tokens: &PersistedTokens) -> PersistedTokens {
    mark_tokens_lifecycle_published_inner(tokens, None, None)
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
    Ok(mark_tokens_lifecycle_published_inner(
        tokens,
        Some(transition.generation()),
        transition.credential_published_at_millis(),
    ))
}

pub fn tokens_lifecycle_published(tokens: &PersistedTokens) -> bool {
    tokens
        .metadata
        .get(TOKEN_LIFECYCLE_METADATA_KEY)
        .and_then(|marker| marker.get("published"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
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
    require_explicit_expiry: bool,
) -> Option<TokenLifecyclePublication> {
    if !tokens_lifecycle_published(tokens) {
        return None;
    }
    let marker = tokens.metadata.get(TOKEN_LIFECYCLE_METADATA_KEY)?;
    let generation = marker.get("generation").and_then(serde_json::Value::as_u64);
    let explicit_expires_at = marker.get("expires_at").and_then(serde_json::Value::as_u64);
    let expires_at = match (explicit_expires_at, require_explicit_expiry) {
        (Some(expires_at), _) => expires_at,
        (None, true) => return None,
        (None, false) => persisted_token_expires_at_epoch_secs(tokens),
    };
    let credential_published_at_millis = marker
        .get("credential_published_at_millis")
        .and_then(serde_json::Value::as_u64);
    Some(TokenLifecyclePublication {
        generation,
        expires_at,
        credential_published_at_millis,
    })
}

pub fn tokens_lifecycle_published_credential_time(tokens: &PersistedTokens) -> Option<u64> {
    tokens
        .metadata
        .get(TOKEN_LIFECYCLE_METADATA_KEY)
        .and_then(|marker| marker.get("credential_published_at_millis"))
        .and_then(serde_json::Value::as_u64)
}

pub fn publish_token_lifecycle_acquired(
    handle: &dyn AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
) -> Result<AuthLeaseTransition, DslTransitionError> {
    let lease_key = LeaseKey::from_auth_binding(auth_binding);
    handle.acquire_lease(&lease_key, persisted_token_expires_at_epoch_secs(tokens))
}

pub fn publish_token_lifecycle_released(
    handle: &dyn AuthLeaseHandle,
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
    #[error("TokenStore load failed: {load_error}; TokenStore clear failed: {clear_error}")]
    TokenStoreLoadAndClear {
        load_error: TokenStoreError,
        clear_error: TokenStoreError,
    },
    #[error(
        "TokenStore clear failed: {clear_error}; AuthMachine lifecycle restore failed: {restore_error}"
    )]
    TokenStoreClearAndLifecycleRestore {
        clear_error: TokenStoreError,
        restore_error: DslTransitionError,
    },
}

/// Clear persisted token material and release the AuthMachine lifecycle as one
/// fail-closed boundary.
///
/// When the previous token snapshot can be loaded, the AuthMachine release
/// happens first. If the token clear then fails, the previous lifecycle snapshot
/// is restored so public status does not commit a split "token exists but
/// lifecycle is gone" state. When token material is unreadable, there is no
/// durable token snapshot to restore, so release is delayed until clear succeeds.
pub async fn clear_tokens_and_publish_lifecycle_released(
    store: &dyn TokenStore,
    handle: &dyn AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
) -> Result<(), TokenLifecycleClearError> {
    let key = TokenKey::from_auth_binding(auth_binding);
    let lease_key = LeaseKey::from_auth_binding(auth_binding);
    let previous_lifecycle = handle.capture_auth_lifecycle_restore_snapshot(&lease_key);
    let previous = match store.load(&key).await {
        Ok(previous) => previous,
        Err(load_error) => {
            if let Err(clear_error) = store.clear(&key).await {
                return Err(TokenLifecycleClearError::TokenStoreLoadAndClear {
                    load_error,
                    clear_error,
                });
            }
            publish_token_lifecycle_released(handle, auth_binding)
                .map_err(TokenLifecycleClearError::AuthMachineRelease)?;
            return Ok(());
        }
    };
    publish_token_lifecycle_released(handle, auth_binding)
        .map_err(TokenLifecycleClearError::AuthMachineRelease)?;
    if let Err(clear_error) = store.clear(&key).await {
        let _ = previous;
        if let Err(restore_error) = restore_token_lifecycle_snapshot(handle, &previous_lifecycle) {
            return Err(
                TokenLifecycleClearError::TokenStoreClearAndLifecycleRestore {
                    clear_error,
                    restore_error,
                },
            );
        }
        return Err(TokenLifecycleClearError::TokenStoreClear(clear_error));
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
    handle: &dyn AuthLeaseHandle,
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
    #[error("AuthMachine lifecycle rollback failed after token marker save failure: {0}")]
    LifecycleRollback(DslTransitionError),
    #[error("TokenStore lifecycle marker save failed: {0}")]
    MarkerSave(TokenStoreError),
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn rehydrate_marked_oauth_tokens_for_status(
    _token_store: &dyn TokenStore,
    _auth_lease: &dyn AuthLeaseHandle,
    _auth_binding: &AuthBindingRef,
    _expected_mode: PersistedAuthMode,
    _now: DateTime<Utc>,
) -> Result<Option<PersistedTokens>, AuthStatusRehydrateError> {
    // Lifecycle markers in persisted OAuth material are projection data only.
    // Auth status must not recreate an AuthMachine credential lease from token
    // JSON, otherwise durable material can shadow machine-owned lease truth.
    Ok(None)
}

pub fn project_published_auth_status<'a>(
    now: DateTime<Utc>,
    stored: Option<&'a PersistedTokens>,
    snapshot: &AuthLeaseSnapshot,
) -> PublishedAuthStatus<'a> {
    let phase = AuthStatusPhase::from_lease_snapshot(now, snapshot);
    if phase == AuthStatusPhase::Unknown {
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

    #[test]
    fn lifecycle_marker_is_only_added_to_oauth_login_tokens() {
        let api_key = PersistedTokens::api_key("sk-test");
        assert_eq!(mark_tokens_lifecycle_published(&api_key), api_key);

        let oauth = oauth_tokens_with_metadata(serde_json::Value::Null);
        let marked = mark_tokens_lifecycle_published(&oauth);
        assert!(tokens_lifecycle_published(&marked));
        assert!(!tokens_lifecycle_published(&oauth));
    }

    #[test]
    fn lifecycle_marker_preserves_existing_object_metadata() {
        let oauth = oauth_tokens_with_metadata(serde_json::json!({
            "provider": "openai",
        }));

        let marked = mark_tokens_lifecycle_published(&oauth);

        assert!(tokens_lifecycle_published(&marked));
        assert_eq!(marked.metadata["provider"], "openai");
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
