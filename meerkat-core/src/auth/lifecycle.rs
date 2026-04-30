//! Auth lifecycle publication helpers.
//!
//! TokenStore owns credential material. AuthMachine owns lifecycle state.
//! Surfaces that write or clear credentials use these helpers so the public
//! status path observes the machine-owned lease instead of deriving phase from
//! persisted token bytes.

use chrono::{DateTime, Utc};
use thiserror::Error;

use super::status::AuthStatusPhase;
use super::token_store::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};
use crate::connection::ConnectionRef;
use crate::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, DslTransitionError, LeaseKey,
};

pub fn persisted_token_expires_at_epoch_secs(tokens: &PersistedTokens) -> u64 {
    tokens
        .expires_at
        .map(|ts| ts.timestamp().max(0) as u64)
        .unwrap_or(u64::MAX)
}

pub fn publish_token_lifecycle_acquired(
    handle: &dyn AuthLeaseHandle,
    connection_ref: &ConnectionRef,
    tokens: &PersistedTokens,
) -> Result<(), DslTransitionError> {
    let lease_key = LeaseKey::from_connection_ref(connection_ref);
    handle.acquire_lease(&lease_key, persisted_token_expires_at_epoch_secs(tokens))?;
    Ok(())
}

pub fn publish_token_lifecycle_released(
    handle: &dyn AuthLeaseHandle,
    connection_ref: &ConnectionRef,
) -> Result<(), DslTransitionError> {
    let lease_key = LeaseKey::from_connection_ref(connection_ref);
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
    connection_ref: &ConnectionRef,
) -> Result<(), TokenLifecycleClearError> {
    let key = TokenKey::from_connection_ref(connection_ref);
    let lease_key = LeaseKey::from_connection_ref(connection_ref);
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
            publish_token_lifecycle_released(handle, connection_ref)
                .map_err(TokenLifecycleClearError::AuthMachineRelease)?;
            return Ok(());
        }
    };
    publish_token_lifecycle_released(handle, connection_ref)
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

fn restore_token_lifecycle_snapshot(
    handle: &dyn AuthLeaseHandle,
    lease_key: &LeaseKey,
    snapshot: &AuthLeaseSnapshot,
    previous: Option<&PersistedTokens>,
) -> Result<(), DslTransitionError> {
    let Some(phase) = snapshot.phase else {
        return Ok(());
    };
    if phase == AuthLeasePhase::Released {
        return Ok(());
    }

    let expires_at = snapshot
        .expires_at
        .or_else(|| previous.map(persisted_token_expires_at_epoch_secs))
        .unwrap_or(u64::MAX);
    handle.acquire_lease(lease_key, expires_at)?;
    match phase {
        AuthLeasePhase::Valid => Ok(()),
        AuthLeasePhase::Expiring => handle.mark_expiring(lease_key),
        AuthLeasePhase::Refreshing => handle.begin_refresh(lease_key),
        AuthLeasePhase::ReauthRequired => handle.mark_reauth_required(lease_key),
        AuthLeasePhase::Released => Ok(()),
    }
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
            Ok(AuthLeaseTransition { generation: 1 })
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
            Ok(AuthLeaseTransition { generation: 1 })
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
                generation: 1,
            }
        }
    }

    fn connection_ref() -> ConnectionRef {
        ConnectionRef {
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
        let connection_ref = connection_ref();

        publish_token_lifecycle_acquired(&handle, &connection_ref, &tokens).unwrap();

        assert_eq!(
            handle.acquired(),
            vec![(
                LeaseKey::from_connection_ref(&connection_ref),
                1_800_000_000
            )]
        );
    }

    #[test]
    fn lifecycle_acquire_maps_non_expiring_tokens_to_unbounded_lease() {
        let handle = RecordingAuthLeaseHandle::default();
        let tokens = tokens_with_expiry(None);

        publish_token_lifecycle_acquired(&handle, &connection_ref(), &tokens).unwrap();

        assert_eq!(handle.acquired()[0].1, u64::MAX);
    }

    #[tokio::test]
    async fn clear_boundary_restores_lifecycle_when_token_clear_fails() {
        let handle = RecordingAuthLeaseHandle::default();
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let tokens = tokens_with_expiry(Some(expires_at));
        let store = ClearFailingTokenStore::new(tokens.clone());
        let connection_ref = connection_ref();

        let err = clear_tokens_and_publish_lifecycle_released(&store, &handle, &connection_ref)
            .await
            .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreClear(_)));
        assert_eq!(
            handle.released(),
            vec![LeaseKey::from_connection_ref(&connection_ref)]
        );
        assert_eq!(
            handle.acquired(),
            vec![(
                LeaseKey::from_connection_ref(&connection_ref),
                persisted_token_expires_at_epoch_secs(&tokens),
            )]
        );
        assert!(
            store
                .load(&TokenKey::from_connection_ref(&connection_ref))
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn clear_boundary_allows_clear_when_previous_token_load_fails() {
        let handle = RecordingAuthLeaseHandle::default();
        let store = LoadFailingTokenStore::new();
        let connection_ref = connection_ref();

        clear_tokens_and_publish_lifecycle_released(&store, &handle, &connection_ref)
            .await
            .unwrap();

        assert_eq!(
            handle.released(),
            vec![LeaseKey::from_connection_ref(&connection_ref)]
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
        let connection_ref = connection_ref();

        let err = clear_tokens_and_publish_lifecycle_released(&store, &handle, &connection_ref)
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
            generation: 1,
        };

        let status = project_published_auth_status(now, None, &snapshot);

        assert_eq!(status.phase, AuthStatusPhase::Valid);
        assert!(status.expires_at.is_some());
        assert!(status.tokens.is_none());
    }
}
