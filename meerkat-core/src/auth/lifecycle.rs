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
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition, DslTransitionError,
    LeaseKey,
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

pub fn mark_tokens_lifecycle_published(tokens: &PersistedTokens) -> PersistedTokens {
    mark_tokens_lifecycle_published_inner(tokens, None, None)
}

pub fn mark_tokens_lifecycle_published_for_generation(
    tokens: &PersistedTokens,
    generation: u64,
) -> PersistedTokens {
    mark_tokens_lifecycle_published_inner(tokens, Some(generation), None)
}

pub fn mark_tokens_lifecycle_published_for_transition(
    tokens: &PersistedTokens,
    transition: AuthLeaseTransition,
) -> PersistedTokens {
    mark_tokens_lifecycle_published_inner(
        tokens,
        Some(transition.generation),
        transition.credential_published_at_millis,
    )
}

pub fn mark_tokens_lifecycle_published_for_snapshot(
    tokens: &PersistedTokens,
    snapshot: &AuthLeaseSnapshot,
) -> PersistedTokens {
    mark_tokens_lifecycle_published_inner(
        tokens,
        Some(snapshot.generation),
        snapshot.credential_published_at_millis,
    )
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
    let previous_lifecycle = handle.snapshot(&lease_key);
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
        if let Err(restore_error) = restore_token_lifecycle_snapshot(
            handle,
            &lease_key,
            &previous_lifecycle,
            previous.as_ref(),
        ) {
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
    lease_key: &LeaseKey,
    snapshot: &AuthLeaseSnapshot,
    previous: Option<&PersistedTokens>,
) -> Result<(), DslTransitionError> {
    if !snapshot.credential_present {
        return Ok(());
    }
    let Some(phase) = snapshot.phase else {
        return Ok(());
    };
    if phase == AuthLeasePhase::Released {
        return Ok(());
    }

    let Some(expires_at) = snapshot
        .expires_at
        .or_else(|| previous.map(persisted_token_expires_at_epoch_secs))
    else {
        return Ok(());
    };
    handle.restore_auth_lifecycle_snapshot(lease_key, snapshot, Some(expires_at))
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
    if !snapshot.credential_present {
        return None;
    }
    let publication = tokens_lifecycle_publication_with_explicit_expiry(tokens)?;
    if publication.expires_at != persisted_token_expires_at_epoch_secs(tokens) {
        return None;
    }
    let snapshot_published_at = snapshot.credential_published_at_millis?;
    let token_published_at = publication.credential_published_at_millis?;
    if token_published_at < snapshot_published_at {
        return None;
    }
    if token_published_at == snapshot_published_at {
        return None;
    }
    Some(AuthLeaseSnapshot {
        phase: Some(AuthLeasePhase::Valid),
        expires_at: (publication.expires_at != u64::MAX).then_some(publication.expires_at),
        credential_present: true,
        generation: publication.generation.unwrap_or(snapshot.generation),
        credential_published_at_millis: Some(token_published_at),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::auth::PersistedAuthMode;
    use crate::connection::{BindingId, RealmId};
    use crate::handles::{AuthLeasePhase, AuthLeaseTransition, DslTransitionError};
    use async_trait::async_trait;

    #[derive(Default)]
    struct RecordingAuthLeaseHandle {
        acquired: Mutex<Vec<(LeaseKey, u64)>>,
        released: Mutex<Vec<LeaseKey>>,
    }

    impl RecordingAuthLeaseHandle {
        fn acquired(&self) -> Vec<(LeaseKey, u64)> {
            self.acquired
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn released(&self) -> Vec<LeaseKey> {
            self.released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl AuthLeaseHandle for RecordingAuthLeaseHandle {
        fn acquire_lease(
            &self,
            lease_key: &LeaseKey,
            expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            self.acquired
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((lease_key.clone(), expires_at));
            Ok(AuthLeaseTransition {
                generation: 1,
                credential_published_at_millis: None,
            })
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            Ok(AuthLeaseTransition {
                generation: 1,
                credential_published_at_millis: None,
            })
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn release_lease(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            self.released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(lease_key.clone());
            Ok(())
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: None,
                credential_present: true,
                generation: 1,
                credential_published_at_millis: None,
            }
        }
    }

    fn auth_binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse("dev").expect("valid realm"),
            binding: BindingId::parse("default_openai").expect("valid binding"),
            profile: None,
        }
    }

    fn tokens_with_expiry(expires_at: Option<DateTime<Utc>>) -> PersistedTokens {
        PersistedTokens {
            auth_mode: PersistedAuthMode::ApiKey,
            primary_secret: Some("secret".into()),
            refresh_token: None,
            id_token: None,
            expires_at,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

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

    struct ClearFailingTokenStore {
        tokens: Mutex<Option<PersistedTokens>>,
    }

    impl ClearFailingTokenStore {
        fn new(tokens: PersistedTokens) -> Self {
            Self {
                tokens: Mutex::new(Some(tokens)),
            }
        }
    }

    struct LoadFailingTokenStore {
        cleared: Mutex<bool>,
        clear_error: bool,
    }

    impl LoadFailingTokenStore {
        fn new() -> Self {
            Self {
                cleared: Mutex::new(false),
                clear_error: false,
            }
        }

        fn new_with_clear_error() -> Self {
            Self {
                cleared: Mutex::new(false),
                clear_error: true,
            }
        }

        fn cleared(&self) -> bool {
            *self
                .cleared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    #[async_trait]
    impl TokenStore for LoadFailingTokenStore {
        async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            Err(TokenStoreError::Serde("corrupt token".into()))
        }

        async fn save(
            &self,
            _key: &TokenKey,
            _tokens: &PersistedTokens,
        ) -> Result<(), TokenStoreError> {
            Ok(())
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            *self
                .cleared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            if self.clear_error {
                Err(TokenStoreError::Unavailable("clear unavailable".into()))
            } else {
                Ok(())
            }
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "load_failing"
        }
    }

    #[async_trait]
    impl TokenStore for ClearFailingTokenStore {
        async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            Ok(self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }

        async fn save(
            &self,
            _key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), TokenStoreError> {
            *self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tokens.clone());
            Ok(())
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            Err(TokenStoreError::Unavailable("clear unavailable".into()))
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "clear_failing"
        }
    }

    #[test]
    fn lifecycle_acquire_uses_persisted_token_expiry_as_machine_input() {
        let handle = RecordingAuthLeaseHandle::default();
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let tokens = tokens_with_expiry(Some(expires_at));
        let auth_binding = auth_binding();

        publish_token_lifecycle_acquired(&handle, &auth_binding, &tokens).unwrap();

        assert_eq!(
            handle.acquired(),
            vec![(LeaseKey::from_auth_binding(&auth_binding), 1_800_000_000)]
        );
    }

    #[test]
    fn lifecycle_acquire_maps_non_expiring_tokens_to_unbounded_lease() {
        let handle = RecordingAuthLeaseHandle::default();
        let tokens = tokens_with_expiry(None);

        publish_token_lifecycle_acquired(&handle, &auth_binding(), &tokens).unwrap();

        assert_eq!(handle.acquired()[0].1, u64::MAX);
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

    #[tokio::test]
    async fn clear_boundary_restores_lifecycle_when_token_clear_fails() {
        let handle = RecordingAuthLeaseHandle::default();
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let tokens = tokens_with_expiry(Some(expires_at));
        let store = ClearFailingTokenStore::new(tokens.clone());
        let auth_binding = auth_binding();

        let err = clear_tokens_and_publish_lifecycle_released(&store, &handle, &auth_binding)
            .await
            .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreClear(_)));
        assert_eq!(
            handle.released(),
            vec![LeaseKey::from_auth_binding(&auth_binding)]
        );
        assert_eq!(
            handle.acquired(),
            vec![(
                LeaseKey::from_auth_binding(&auth_binding),
                persisted_token_expires_at_epoch_secs(&tokens),
            )]
        );
        assert!(
            store
                .load(&TokenKey::from_auth_binding(&auth_binding))
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn clear_boundary_allows_clear_when_previous_token_load_fails() {
        let handle = RecordingAuthLeaseHandle::default();
        let store = LoadFailingTokenStore::new();
        let auth_binding = auth_binding();

        clear_tokens_and_publish_lifecycle_released(&store, &handle, &auth_binding)
            .await
            .unwrap();

        assert_eq!(
            handle.released(),
            vec![LeaseKey::from_auth_binding(&auth_binding)]
        );
        assert!(store.cleared());
        assert!(
            handle.acquired().is_empty(),
            "unreadable previous tokens cannot be used to restore a lease"
        );
    }

    #[tokio::test]
    async fn clear_boundary_does_not_release_lifecycle_when_load_and_clear_fail() {
        let handle = RecordingAuthLeaseHandle::default();
        let store = LoadFailingTokenStore::new_with_clear_error();
        let auth_binding = auth_binding();

        let err = clear_tokens_and_publish_lifecycle_released(&store, &handle, &auth_binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            TokenLifecycleClearError::TokenStoreLoadAndClear { .. }
        ));
        assert!(store.cleared());
        assert!(
            handle.released().is_empty(),
            "lifecycle must remain untouched when unreadable token material cannot be cleared"
        );
        assert!(handle.acquired().is_empty());
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
