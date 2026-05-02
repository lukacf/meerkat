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

pub fn persisted_token_acquired_snapshot(
    tokens: &PersistedTokens,
    generation: u64,
) -> AuthLeaseSnapshot {
    let expires_at = match persisted_token_expires_at_epoch_secs(tokens) {
        u64::MAX => None,
        expires_at => Some(expires_at),
    };
    AuthLeaseSnapshot {
        phase: Some(AuthLeasePhase::Valid),
        expires_at,
        generation,
    }
}

#[derive(Debug, Error)]
pub enum TokenLifecycleSaveError {
    #[error("TokenStore load failed: {0}")]
    TokenStoreLoad(TokenStoreError),
    #[error("TokenStore changed before pending AuthMachine lease material could be persisted")]
    TokenStorePrepareRace,
    #[error("TokenStore conditional save failed before AuthMachine lifecycle acquire: {0}")]
    TokenStorePrepare(TokenStoreError),
    #[error("AuthMachine refresh ownership is already held by another actor")]
    AuthMachineRefreshInProgress,
    #[error("AuthMachine lifecycle begin_refresh lost a race")]
    AuthMachineBeginRefreshRace,
    #[error("AuthMachine lifecycle begin_refresh failed: {0}")]
    AuthMachineBeginRefresh(DslTransitionError),
    #[error(
        "AuthMachine lifecycle acquire lost a race; pending_token_restored={pending_token_restored}"
    )]
    AuthMachineAcquireRace { pending_token_restored: bool },
    #[error(
        "AuthMachine lifecycle acquire failed: {source}; pending_token_restored={pending_token_restored}"
    )]
    AuthMachineAcquire {
        source: DslTransitionError,
        pending_token_restored: bool,
    },
    #[error(
        "AuthMachine lifecycle complete_refresh lost a race; pending_token_restored={pending_token_restored}"
    )]
    AuthMachineCompleteRefreshRace { pending_token_restored: bool },
    #[error(
        "AuthMachine lifecycle complete_refresh failed: {source}; pending_token_restored={pending_token_restored}; rollback_marked={rollback_marked}"
    )]
    AuthMachineCompleteRefresh {
        source: DslTransitionError,
        pending_token_restored: bool,
        rollback_marked: bool,
    },
    #[error("AuthMachine lifecycle refresh_failed rollback failed: {0}")]
    AuthMachineRefreshRollback(DslTransitionError),
    #[error("TokenStore restore failed after AuthMachine lifecycle acquire failed: {0}")]
    TokenStoreRestore(TokenStoreError),
    #[error(
        "TokenStore material changed before bound AuthMachine lease material could be finalized"
    )]
    TokenStoreFinalizeRace,
    #[error("TokenStore conditional save failed while finalizing bound AuthMachine lease: {0}")]
    TokenStoreFinalize(TokenStoreError),
    #[error("TokenStore load failed while checking bound AuthMachine lease material: {0}")]
    TokenStoreFinalizeLoad(TokenStoreError),
    #[error(
        "AuthMachine lifecycle changed after token save; stale_token_cleared={stale_token_cleared}"
    )]
    AuthMachineChangedAfterSave { stale_token_cleared: bool },
    #[error("AuthMachine lifecycle changed after token save; stale TokenStore cleanup failed: {0}")]
    TokenStoreCleanup(TokenStoreError),
}

pub async fn save_tokens_and_publish_lifecycle_acquired(
    store: &dyn TokenStore,
    handle: &dyn AuthLeaseHandle,
    connection_ref: &ConnectionRef,
    tokens: &PersistedTokens,
) -> Result<(), TokenLifecycleSaveError> {
    let tokens = tokens.clone().canonicalize_for_persistence();
    let key = TokenKey::from_connection_ref(connection_ref);
    let lease_key = LeaseKey::from_connection_ref(connection_ref);
    let previous = match store.load(&key).await {
        Ok(previous) => previous,
        Err(TokenStoreError::KeyringUnavailable(_)) => None,
        Err(err) => return Err(TokenLifecycleSaveError::TokenStoreLoad(err)),
    };
    let previous_lifecycle = handle.snapshot(&lease_key);
    if matches!(
        previous_lifecycle.phase,
        Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
    ) {
        return save_tokens_through_refreshing_lifecycle(
            store,
            handle,
            &key,
            &lease_key,
            previous.as_ref(),
            &previous_lifecycle,
            &tokens,
        )
        .await;
    }
    if previous_lifecycle.phase == Some(AuthLeasePhase::Refreshing) {
        return Err(TokenLifecycleSaveError::AuthMachineRefreshInProgress);
    }

    save_tokens_through_acquire_lifecycle(
        store,
        handle,
        &key,
        &lease_key,
        previous.as_ref(),
        &previous_lifecycle,
        &tokens,
    )
    .await
}

async fn save_tokens_through_acquire_lifecycle(
    store: &dyn TokenStore,
    handle: &dyn AuthLeaseHandle,
    key: &TokenKey,
    lease_key: &LeaseKey,
    previous: Option<&PersistedTokens>,
    previous_lifecycle: &AuthLeaseSnapshot,
    tokens: &PersistedTokens,
) -> Result<(), TokenLifecycleSaveError> {
    let pending_tokens = tokens
        .clone()
        .with_auth_pending_owner_binding(key.clone(), previous_lifecycle.generation);
    match store
        .save_if_current_optional(key, previous, &pending_tokens)
        .await
    {
        Ok(true) => {}
        Ok(false) => return Err(TokenLifecycleSaveError::TokenStorePrepareRace),
        Err(err) => return Err(TokenLifecycleSaveError::TokenStorePrepare(err)),
    }

    let transition = match handle.acquire_lease_if_snapshot(
        lease_key,
        previous_lifecycle,
        persisted_token_expires_at_epoch_secs(tokens),
    ) {
        Ok(Some(transition)) => transition,
        Ok(None) => {
            let pending_token_restored =
                restore_pending_tokens_if_current(store, key, &pending_tokens, previous).await?;
            return Err(TokenLifecycleSaveError::AuthMachineAcquireRace {
                pending_token_restored,
            });
        }
        Err(source) => {
            let pending_token_restored =
                restore_pending_tokens_if_current(store, key, &pending_tokens, previous).await?;
            return Err(TokenLifecycleSaveError::AuthMachineAcquire {
                source,
                pending_token_restored,
            });
        }
    };

    let acquired_snapshot = persisted_token_acquired_snapshot(tokens, transition.generation);
    let bound_tokens = tokens
        .clone()
        .with_auth_lease_binding(key.clone(), transition.generation);
    finalize_saved_tokens_for_snapshot(
        store,
        handle,
        key,
        lease_key,
        &pending_tokens,
        &bound_tokens,
        &acquired_snapshot,
    )
    .await
}

async fn save_tokens_through_refreshing_lifecycle(
    store: &dyn TokenStore,
    handle: &dyn AuthLeaseHandle,
    key: &TokenKey,
    lease_key: &LeaseKey,
    previous: Option<&PersistedTokens>,
    previous_lifecycle: &AuthLeaseSnapshot,
    tokens: &PersistedTokens,
) -> Result<(), TokenLifecycleSaveError> {
    let refresh_transition = match handle.begin_refresh_if_snapshot(lease_key, previous_lifecycle) {
        Ok(Some(transition)) => transition,
        Ok(None) => return Err(TokenLifecycleSaveError::AuthMachineBeginRefreshRace),
        Err(err) => return Err(TokenLifecycleSaveError::AuthMachineBeginRefresh(err)),
    };
    let refreshing_snapshot = AuthLeaseSnapshot {
        phase: Some(AuthLeasePhase::Refreshing),
        expires_at: previous_lifecycle.expires_at,
        generation: refresh_transition.generation,
    };
    let pending_tokens = tokens
        .clone()
        .with_auth_pending_owner_binding(key.clone(), refreshing_snapshot.generation);
    match store
        .save_if_current_optional(key, previous, &pending_tokens)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            rollback_refreshing_lifecycle(handle, lease_key, &refreshing_snapshot, false)?;
            return Err(TokenLifecycleSaveError::TokenStorePrepareRace);
        }
        Err(err) => {
            rollback_refreshing_lifecycle(handle, lease_key, &refreshing_snapshot, false)?;
            return Err(TokenLifecycleSaveError::TokenStorePrepare(err));
        }
    }

    let transition = match handle.complete_refresh_if_snapshot(
        lease_key,
        &refreshing_snapshot,
        persisted_token_expires_at_epoch_secs(tokens),
        epoch_secs(Utc::now()),
    ) {
        Ok(Some(transition)) => transition,
        Ok(None) => {
            let pending_token_restored =
                restore_pending_tokens_if_current(store, key, &pending_tokens, previous).await?;
            return Err(TokenLifecycleSaveError::AuthMachineCompleteRefreshRace {
                pending_token_restored,
            });
        }
        Err(source) => {
            let pending_token_restored =
                restore_pending_tokens_if_current(store, key, &pending_tokens, previous).await?;
            let rollback_marked =
                rollback_refreshing_lifecycle(handle, lease_key, &refreshing_snapshot, false)?;
            return Err(TokenLifecycleSaveError::AuthMachineCompleteRefresh {
                source,
                pending_token_restored,
                rollback_marked,
            });
        }
    };

    let acquired_snapshot = persisted_token_acquired_snapshot(tokens, transition.generation);
    let bound_tokens = tokens
        .clone()
        .with_auth_lease_binding(key.clone(), transition.generation);
    finalize_saved_tokens_for_snapshot(
        store,
        handle,
        key,
        lease_key,
        &pending_tokens,
        &bound_tokens,
        &acquired_snapshot,
    )
    .await
}

fn rollback_refreshing_lifecycle(
    handle: &dyn AuthLeaseHandle,
    lease_key: &LeaseKey,
    snapshot: &AuthLeaseSnapshot,
    permanent: bool,
) -> Result<bool, TokenLifecycleSaveError> {
    handle
        .refresh_failed_if_snapshot(lease_key, snapshot, permanent)
        .map_err(TokenLifecycleSaveError::AuthMachineRefreshRollback)
}

async fn finalize_saved_tokens_for_snapshot(
    store: &dyn TokenStore,
    handle: &dyn AuthLeaseHandle,
    key: &TokenKey,
    lease_key: &LeaseKey,
    pending_tokens: &PersistedTokens,
    bound_tokens: &PersistedTokens,
    acquired_snapshot: &AuthLeaseSnapshot,
) -> Result<(), TokenLifecycleSaveError> {
    match store
        .save_if_current(key, pending_tokens, bound_tokens)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            if !stored_tokens_match_or_mark_reauth(store, handle, key, lease_key, acquired_snapshot)
                .await?
            {
                return Err(TokenLifecycleSaveError::TokenStoreFinalizeRace);
            }
        }
        Err(err) => {
            let _ = handle.mark_reauth_required_if_snapshot(lease_key, acquired_snapshot);
            store
                .clear_if_current(key, bound_tokens)
                .await
                .map_err(TokenLifecycleSaveError::TokenStoreCleanup)?;
            return Err(TokenLifecycleSaveError::TokenStoreFinalize(err));
        }
    }

    if handle.snapshot(lease_key) != *acquired_snapshot {
        let cleared_bound = store
            .clear_if_current(key, bound_tokens)
            .await
            .map_err(TokenLifecycleSaveError::TokenStoreCleanup)?;
        let cleared_pending = if cleared_bound {
            false
        } else {
            store
                .clear_if_current(key, pending_tokens)
                .await
                .map_err(TokenLifecycleSaveError::TokenStoreCleanup)?
        };
        return Err(TokenLifecycleSaveError::AuthMachineChangedAfterSave {
            stale_token_cleared: cleared_bound || cleared_pending,
        });
    }

    Ok(())
}

fn epoch_secs(ts: DateTime<Utc>) -> u64 {
    ts.timestamp().max(0) as u64
}

async fn restore_pending_tokens_if_current(
    store: &dyn TokenStore,
    key: &TokenKey,
    pending_tokens: &PersistedTokens,
    previous: Option<&PersistedTokens>,
) -> Result<bool, TokenLifecycleSaveError> {
    match previous {
        Some(previous) => store
            .save_if_current(key, pending_tokens, previous)
            .await
            .map_err(TokenLifecycleSaveError::TokenStoreRestore),
        None => store
            .clear_if_current(key, pending_tokens)
            .await
            .map_err(TokenLifecycleSaveError::TokenStoreRestore),
    }
}

async fn stored_tokens_match_lifecycle_snapshot(
    store: &dyn TokenStore,
    key: &TokenKey,
    snapshot: &AuthLeaseSnapshot,
) -> Result<bool, TokenLifecycleSaveError> {
    Ok(store
        .load(key)
        .await
        .map_err(TokenLifecycleSaveError::TokenStoreFinalizeLoad)?
        .as_ref()
        .is_some_and(|tokens| persisted_tokens_match_lifecycle_snapshot(tokens, key, snapshot)))
}

async fn stored_tokens_match_or_mark_reauth(
    store: &dyn TokenStore,
    handle: &dyn AuthLeaseHandle,
    key: &TokenKey,
    lease_key: &LeaseKey,
    snapshot: &AuthLeaseSnapshot,
) -> Result<bool, TokenLifecycleSaveError> {
    match stored_tokens_match_lifecycle_snapshot(store, key, snapshot).await {
        Ok(true) => Ok(true),
        Ok(false) => {
            let _ = handle.mark_reauth_required_if_snapshot(lease_key, snapshot);
            Ok(false)
        }
        Err(err) => {
            let _ = handle.mark_reauth_required_if_snapshot(lease_key, snapshot);
            Err(err)
        }
    }
}

pub fn persisted_tokens_match_lifecycle_snapshot(
    tokens: &PersistedTokens,
    key: &TokenKey,
    snapshot: &AuthLeaseSnapshot,
) -> bool {
    persisted_token_expires_at_epoch_secs(tokens) == snapshot.expires_at.unwrap_or(u64::MAX)
        && tokens.auth_lease.as_ref().is_some_and(|binding| {
            binding.token_key == *key
                && binding.pending_owner_generation.is_none()
                && binding.generation == snapshot.generation
        })
}

#[derive(Debug, Error)]
pub enum TokenLifecycleClearError {
    #[error("AuthMachine lifecycle release failed: {0}")]
    AuthMachineRelease(DslTransitionError),
    #[error("TokenStore load failed: {0}")]
    TokenStoreLoad(TokenStoreError),
    #[error("TokenStore clear failed: {0}")]
    TokenStoreClear(TokenStoreError),
    #[error("TokenStore material changed before AuthMachine lifecycle release")]
    TokenStoreClearRace,
    #[error(
        "AuthMachine lifecycle release failed after TokenStore clear: {release_error}; TokenStore restore failed: {restore_error}"
    )]
    TokenStoreRestoreAfterAuthMachineRelease {
        release_error: DslTransitionError,
        restore_error: TokenStoreError,
    },
}

/// Clear persisted token material and release the AuthMachine lifecycle as one
/// fail-closed boundary.
///
/// When the previous token snapshot can be loaded, the TokenStore conditional
/// clear must win before the AuthMachine release is attempted. `Ok(false)` from
/// the clear path means newer durable material exists, so the lease remains
/// untouched and the caller sees a race instead of a false logout success.
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
        Err(load_error) => return Err(TokenLifecycleClearError::TokenStoreLoad(load_error)),
    };
    let Some(previous_tokens) = previous.as_ref() else {
        let released = handle
            .release_lease_if_snapshot(&lease_key, &previous_lifecycle)
            .map_err(TokenLifecycleClearError::AuthMachineRelease)?;
        if released {
            return Ok(());
        }
        if store
            .load(&key)
            .await
            .map_err(TokenLifecycleClearError::TokenStoreLoad)?
            .is_some()
        {
            return Err(TokenLifecycleClearError::TokenStoreClearRace);
        }
        if handle.snapshot(&lease_key).phase.is_none() {
            return Ok(());
        }
        return Err(TokenLifecycleClearError::TokenStoreClearRace);
    };
    match store.clear_if_current(&key, previous_tokens).await {
        Ok(true) => {}
        Ok(false) => return Err(TokenLifecycleClearError::TokenStoreClearRace),
        Err(clear_error) => return Err(TokenLifecycleClearError::TokenStoreClear(clear_error)),
    }

    match handle.release_lease_if_snapshot(&lease_key, &previous_lifecycle) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(release_error) => {
            restore_cleared_tokens_after_release_error(
                store,
                &key,
                previous_tokens,
                release_error.clone(),
            )
            .await?;
            return Err(TokenLifecycleClearError::AuthMachineRelease(release_error));
        }
    }

    if store
        .load(&key)
        .await
        .map_err(TokenLifecycleClearError::TokenStoreLoad)?
        .is_some()
    {
        return Err(TokenLifecycleClearError::TokenStoreClearRace);
    }

    let current_lifecycle = handle.snapshot(&lease_key);
    if current_lifecycle.phase.is_some() {
        if !snapshot_represents_cleared_token_material(previous_tokens, &key, &current_lifecycle) {
            restore_cleared_tokens_after_lost_release(
                store,
                &key,
                previous_tokens,
                &previous_lifecycle,
                &current_lifecycle,
            )
            .await?;
            return Err(TokenLifecycleClearError::TokenStoreClearRace);
        }
        let released = match handle.release_lease_if_snapshot(&lease_key, &current_lifecycle) {
            Ok(released) => released,
            Err(release_error) => {
                restore_cleared_tokens_after_release_error(
                    store,
                    &key,
                    previous_tokens,
                    release_error.clone(),
                )
                .await?;
                return Err(TokenLifecycleClearError::AuthMachineRelease(release_error));
            }
        };
        if !released {
            if store
                .load(&key)
                .await
                .map_err(TokenLifecycleClearError::TokenStoreLoad)?
                .is_some()
            {
                return Err(TokenLifecycleClearError::TokenStoreClearRace);
            }
            let latest_lifecycle = handle.snapshot(&lease_key);
            if latest_lifecycle.phase.is_none() {
                return Ok(());
            }
            restore_cleared_tokens_after_lost_release(
                store,
                &key,
                previous_tokens,
                &previous_lifecycle,
                &latest_lifecycle,
            )
            .await?;
            return Err(TokenLifecycleClearError::TokenStoreClearRace);
        }
    }
    Ok(())
}

fn snapshot_represents_cleared_token_material(
    tokens: &PersistedTokens,
    key: &TokenKey,
    snapshot: &AuthLeaseSnapshot,
) -> bool {
    matches!(
        snapshot.phase,
        Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
    ) && persisted_tokens_match_lifecycle_snapshot(tokens, key, snapshot)
}

async fn restore_cleared_tokens_after_release_error(
    store: &dyn TokenStore,
    key: &TokenKey,
    previous_tokens: &PersistedTokens,
    release_error: DslTransitionError,
) -> Result<(), TokenLifecycleClearError> {
    store
        .save_if_current_optional(key, None, previous_tokens)
        .await
        .map(|_| ())
        .map_err(|restore_error| {
            TokenLifecycleClearError::TokenStoreRestoreAfterAuthMachineRelease {
                release_error,
                restore_error,
            }
        })
}

async fn restore_cleared_tokens_after_lost_release(
    store: &dyn TokenStore,
    key: &TokenKey,
    previous_tokens: &PersistedTokens,
    previous_lifecycle: &AuthLeaseSnapshot,
    current_lifecycle: &AuthLeaseSnapshot,
) -> Result<(), TokenLifecycleClearError> {
    if !snapshot_can_still_use_cleared_token_material(
        previous_tokens,
        key,
        previous_lifecycle,
        current_lifecycle,
    ) {
        return Ok(());
    }
    store
        .save_if_current_optional(key, None, previous_tokens)
        .await
        .map(|_| ())
        .map_err(TokenLifecycleClearError::TokenStoreClear)
}

fn snapshot_can_still_use_cleared_token_material(
    tokens: &PersistedTokens,
    key: &TokenKey,
    previous_lifecycle: &AuthLeaseSnapshot,
    current_lifecycle: &AuthLeaseSnapshot,
) -> bool {
    snapshot_represents_cleared_token_material(tokens, key, current_lifecycle)
        || (matches!(current_lifecycle.phase, Some(AuthLeasePhase::Refreshing))
            && current_lifecycle.expires_at == previous_lifecycle.expires_at
            && persisted_tokens_match_lifecycle_snapshot(tokens, key, previous_lifecycle))
}

/// Clear unreadable token material and release the AuthMachine lifecycle only
/// when the lifecycle snapshot still matches the pre-clear owner.
pub async fn clear_unreadable_tokens_and_publish_lifecycle_released(
    store: &dyn TokenStore,
    handle: &dyn AuthLeaseHandle,
    connection_ref: &ConnectionRef,
) -> Result<bool, TokenLifecycleClearError> {
    let key = TokenKey::from_connection_ref(connection_ref);
    let lease_key = LeaseKey::from_connection_ref(connection_ref);
    let previous_lifecycle = handle.snapshot(&lease_key);
    let cleared = store
        .clear_if_unreadable(&key)
        .await
        .map_err(TokenLifecycleClearError::TokenStoreClear)?;
    if !cleared {
        return Ok(false);
    }
    let released = handle
        .release_lease_if_snapshot(&lease_key, &previous_lifecycle)
        .map_err(TokenLifecycleClearError::AuthMachineRelease)?;
    if !released {
        if handle.snapshot(&lease_key).phase.is_none()
            && store
                .load(&key)
                .await
                .map_err(TokenLifecycleClearError::TokenStoreLoad)?
                .is_none()
        {
            return Ok(true);
        }
        return Err(TokenLifecycleClearError::TokenStoreClearRace);
    }
    Ok(true)
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
    key: &TokenKey,
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
    let tokens = if matches!(
        snapshot.phase,
        Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
    ) {
        stored.filter(|tokens| persisted_tokens_match_lifecycle_snapshot(tokens, key, snapshot))
    } else {
        None
    };
    PublishedAuthStatus {
        phase,
        expires_at: lease_snapshot_expires_at_datetime(snapshot),
        tokens,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::auth::PersistedAuthMode;
    use crate::connection::{BindingId, RealmId};
    use crate::handles::{AuthLeasePhase, AuthLeaseTransition, DslTransitionError};
    use async_trait::async_trait;

    type ConditionalReleaseHook = Arc<dyn Fn(&LeaseKey, &AuthLeaseSnapshot) + Send + Sync>;

    struct RecordingAuthLeaseHandle {
        acquired: Mutex<Vec<(LeaseKey, u64)>>,
        released: Mutex<Vec<LeaseKey>>,
        snapshot: Mutex<AuthLeaseSnapshot>,
        material_generation: Mutex<u64>,
        reject_release: Mutex<bool>,
        conditional_release_hook: Mutex<Option<ConditionalReleaseHook>>,
    }

    impl Default for RecordingAuthLeaseHandle {
        fn default() -> Self {
            Self {
                acquired: Mutex::new(Vec::new()),
                released: Mutex::new(Vec::new()),
                snapshot: Mutex::new(AuthLeaseSnapshot {
                    phase: Some(AuthLeasePhase::Valid),
                    expires_at: None,
                    generation: 1,
                }),
                material_generation: Mutex::new(1),
                reject_release: Mutex::new(false),
                conditional_release_hook: Mutex::new(None),
            }
        }
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

        fn force_snapshot(&self, snapshot: AuthLeaseSnapshot) {
            if matches!(
                snapshot.phase,
                Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
            ) {
                *self
                    .material_generation
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot.generation;
            }
            *self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot;
        }

        fn reject_release(&self) {
            *self
                .reject_release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        }

        fn set_conditional_release_hook(&self, hook: ConditionalReleaseHook) {
            *self
                .conditional_release_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
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
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            snapshot.phase = Some(AuthLeasePhase::Valid);
            snapshot.expires_at = (expires_at != u64::MAX).then_some(expires_at);
            snapshot.generation += 1;
            *self
                .material_generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot.generation;
            Ok(AuthLeaseTransition {
                generation: snapshot.generation,
            })
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
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            snapshot.phase = Some(AuthLeasePhase::Expiring);
            snapshot.generation = *self
                .material_generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(())
        }

        fn begin_refresh(
            &self,
            _lease_key: &LeaseKey,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !matches!(
                snapshot.phase,
                Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
            ) {
                return Err(DslTransitionError::new(
                    "begin_refresh",
                    "lease is not valid or expiring",
                ));
            }
            snapshot.phase = Some(AuthLeasePhase::Refreshing);
            snapshot.generation += 1;
            Ok(AuthLeaseTransition {
                generation: snapshot.generation,
            })
        }

        fn begin_refresh_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            if self.snapshot(lease_key) != *expected {
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
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if snapshot.phase != Some(AuthLeasePhase::Refreshing) {
                return Err(DslTransitionError::new(
                    "complete_refresh",
                    "lease is not refreshing",
                ));
            }
            snapshot.phase = Some(AuthLeasePhase::Valid);
            snapshot.expires_at = (new_expires_at != u64::MAX).then_some(new_expires_at);
            snapshot.generation += 1;
            *self
                .material_generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot.generation;
            Ok(AuthLeaseTransition {
                generation: snapshot.generation,
            })
        }

        fn complete_refresh_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
            new_expires_at: u64,
            now: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            if self.snapshot(lease_key) != *expected {
                return Ok(None);
            }
            self.complete_refresh(lease_key, new_expires_at, now)
                .map(Some)
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            permanent: bool,
        ) -> Result<(), DslTransitionError> {
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if permanent {
                snapshot.phase = Some(AuthLeasePhase::ReauthRequired);
                snapshot.generation += 1;
                *self
                    .material_generation
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot.generation;
            } else {
                snapshot.phase = Some(AuthLeasePhase::Expiring);
                snapshot.generation = *self
                    .material_generation
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            Ok(())
        }

        fn refresh_failed_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
            permanent: bool,
        ) -> Result<bool, DslTransitionError> {
            if self.snapshot(lease_key) != *expected {
                return Ok(false);
            }
            self.refresh_failed(lease_key, permanent)?;
            Ok(true)
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            snapshot.phase = Some(AuthLeasePhase::ReauthRequired);
            snapshot.generation += 1;
            *self
                .material_generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot.generation;
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

        fn release_lease(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            if *self
                .reject_release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                return Err(DslTransitionError::guard_rejected(
                    "release_lease",
                    "test rejection",
                ));
            }
            self.released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(lease_key.clone());
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            snapshot.phase = None;
            snapshot.expires_at = None;
            snapshot.generation += 1;
            *self
                .material_generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot.generation;
            Ok(())
        }

        fn release_lease_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
        ) -> Result<bool, DslTransitionError> {
            let hook = self
                .conditional_release_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hook) = hook {
                hook(lease_key, expected);
            }
            if self.snapshot(lease_key) != *expected {
                return Ok(false);
            }
            self.release_lease(lease_key)?;
            Ok(true)
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
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
            auth_lease: None,
        }
    }

    struct ClearFailingTokenStore {
        tokens: Mutex<Option<PersistedTokens>>,
        on_clear: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    }

    impl ClearFailingTokenStore {
        fn new(tokens: PersistedTokens) -> Self {
            Self::new_with_on_clear(tokens, None)
        }

        fn new_with_on_clear(
            tokens: PersistedTokens,
            on_clear: Option<Box<dyn Fn() + Send + Sync>>,
        ) -> Self {
            Self {
                tokens: Mutex::new(Some(tokens)),
                on_clear: Mutex::new(on_clear),
            }
        }
    }

    struct LoadFailingTokenStore {
        cleared: Mutex<bool>,
        clear_error: bool,
    }

    struct EmptyTokenStore {
        on_load: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    }

    struct UnreadableClearingTokenStore {
        cleared: Mutex<bool>,
        tokens_after_clear: Mutex<Option<PersistedTokens>>,
        on_clear: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    }

    struct ReplacingOnClearTokenStore {
        tokens: Mutex<Option<PersistedTokens>>,
        replacement: PersistedTokens,
    }

    struct ClearingTokenStore {
        tokens: Mutex<Option<PersistedTokens>>,
        on_clear: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    }

    struct ReplacingAfterLoadTokenStore {
        tokens: Mutex<Option<PersistedTokens>>,
        replacement: PersistedTokens,
        replace_on_next_load: Mutex<bool>,
        on_replace: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    }

    type SaveObserver = Box<dyn Fn(&PersistedTokens) + Send + Sync>;

    struct SaveObservingTokenStore {
        tokens: Mutex<Option<PersistedTokens>>,
        on_save: Mutex<Option<SaveObserver>>,
    }

    struct LoadUnavailableInitialSaveTokenStore {
        tokens: Mutex<Option<PersistedTokens>>,
    }

    struct FinalizeRaceTokenStore {
        tokens: Mutex<Option<PersistedTokens>>,
        fail_load_after_final_save_attempt: bool,
        fail_final_save_after_writing: bool,
        fail_load: Mutex<bool>,
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

    impl EmptyTokenStore {
        fn new_with_on_load(on_load: Option<Box<dyn Fn() + Send + Sync>>) -> Self {
            Self {
                on_load: Mutex::new(on_load),
            }
        }
    }

    impl UnreadableClearingTokenStore {
        fn new() -> Self {
            Self::new_with_on_clear(None)
        }

        fn new_with_on_clear(on_clear: Option<Box<dyn Fn() + Send + Sync>>) -> Self {
            Self::new_with_on_clear_and_tokens_after_clear(on_clear, None)
        }

        fn new_with_on_clear_and_tokens_after_clear(
            on_clear: Option<Box<dyn Fn() + Send + Sync>>,
            tokens_after_clear: Option<PersistedTokens>,
        ) -> Self {
            Self {
                cleared: Mutex::new(false),
                tokens_after_clear: Mutex::new(tokens_after_clear),
                on_clear: Mutex::new(on_clear),
            }
        }

        fn cleared(&self) -> bool {
            *self
                .cleared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    impl ReplacingOnClearTokenStore {
        fn new(previous: PersistedTokens, replacement: PersistedTokens) -> Self {
            Self {
                tokens: Mutex::new(Some(previous)),
                replacement,
            }
        }
    }

    impl ClearingTokenStore {
        fn new(tokens: PersistedTokens, on_clear: Option<Box<dyn Fn() + Send + Sync>>) -> Self {
            Self {
                tokens: Mutex::new(Some(tokens)),
                on_clear: Mutex::new(on_clear),
            }
        }
    }

    impl ReplacingAfterLoadTokenStore {
        fn new(
            replacement: PersistedTokens,
            on_replace: Option<Box<dyn Fn() + Send + Sync>>,
        ) -> Self {
            Self {
                tokens: Mutex::new(None),
                replacement,
                replace_on_next_load: Mutex::new(true),
                on_replace: Mutex::new(on_replace),
            }
        }
    }

    impl SaveObservingTokenStore {
        fn new(on_save: SaveObserver) -> Self {
            Self::new_with_initial(None, on_save)
        }

        fn new_with_initial(initial: Option<PersistedTokens>, on_save: SaveObserver) -> Self {
            Self {
                tokens: Mutex::new(initial),
                on_save: Mutex::new(Some(on_save)),
            }
        }
    }

    impl LoadUnavailableInitialSaveTokenStore {
        fn new() -> Self {
            Self {
                tokens: Mutex::new(None),
            }
        }

        fn stored(&self) -> Option<PersistedTokens> {
            self.tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl FinalizeRaceTokenStore {
        fn new() -> Self {
            Self {
                tokens: Mutex::new(None),
                fail_load_after_final_save_attempt: false,
                fail_final_save_after_writing: false,
                fail_load: Mutex::new(false),
            }
        }

        fn new_with_finalize_load_error() -> Self {
            Self {
                tokens: Mutex::new(None),
                fail_load_after_final_save_attempt: true,
                fail_final_save_after_writing: false,
                fail_load: Mutex::new(false),
            }
        }

        fn new_with_final_save_error_after_write() -> Self {
            Self {
                tokens: Mutex::new(None),
                fail_load_after_final_save_attempt: false,
                fail_final_save_after_writing: true,
                fail_load: Mutex::new(false),
            }
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

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            _expected: &PersistedTokens,
            _replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            Err(TokenStoreError::Serde("corrupt token".into()))
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            _expected: Option<&PersistedTokens>,
            _replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            Err(TokenStoreError::Serde("corrupt token".into()))
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

        async fn clear_if_current(
            &self,
            key: &TokenKey,
            _expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            self.clear(key).await?;
            Ok(true)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "load_failing"
        }
    }

    #[async_trait]
    impl TokenStore for EmptyTokenStore {
        async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            if let Some(on_load) = self
                .on_load
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                on_load();
            }
            Ok(None)
        }

        async fn save(
            &self,
            _key: &TokenKey,
            _tokens: &PersistedTokens,
        ) -> Result<(), TokenStoreError> {
            Ok(())
        }

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            _expected: &PersistedTokens,
            _replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            Ok(false)
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            _expected: Option<&PersistedTokens>,
            _replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            Ok(false)
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            Ok(())
        }

        async fn clear_if_current(
            &self,
            _key: &TokenKey,
            _expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            Ok(false)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "empty"
        }
    }

    #[async_trait]
    impl TokenStore for UnreadableClearingTokenStore {
        async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            if self.cleared() {
                return Ok(self
                    .tokens_after_clear
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone());
            }
            Err(TokenStoreError::Serde("corrupt token".into()))
        }

        async fn save(
            &self,
            _key: &TokenKey,
            _tokens: &PersistedTokens,
        ) -> Result<(), TokenStoreError> {
            Ok(())
        }

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            _expected: &PersistedTokens,
            _replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            Err(TokenStoreError::Serde("corrupt token".into()))
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            _expected: Option<&PersistedTokens>,
            _replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            Err(TokenStoreError::Serde("corrupt token".into()))
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            *self
                .cleared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            Ok(())
        }

        async fn clear_if_unreadable(&self, key: &TokenKey) -> Result<bool, TokenStoreError> {
            if let Some(on_clear) = self
                .on_clear
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                on_clear();
            }
            self.clear(key).await?;
            Ok(true)
        }

        async fn clear_if_current(
            &self,
            _key: &TokenKey,
            _expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            Ok(false)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "unreadable_clearing"
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

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            expected: Option<&PersistedTokens>,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != expected {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            if let Some(on_clear) = self
                .on_clear
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                on_clear();
            }
            Err(TokenStoreError::Unavailable("clear unavailable".into()))
        }

        async fn clear_if_current(
            &self,
            key: &TokenKey,
            expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            if self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                != Some(expected)
            {
                return Ok(false);
            }
            self.clear(key).await?;
            Ok(true)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "clear_failing"
        }
    }

    #[async_trait]
    impl TokenStore for ReplacingOnClearTokenStore {
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

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            expected: Option<&PersistedTokens>,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != expected {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            *self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Ok(())
        }

        async fn clear_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *tokens = Some(self.replacement.clone());
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = None;
            Ok(true)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "replacing_on_clear"
        }
    }

    #[async_trait]
    impl TokenStore for ClearingTokenStore {
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

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            expected: Option<&PersistedTokens>,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != expected {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            *self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Ok(())
        }

        async fn clear_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let cleared = {
                let mut tokens = self
                    .tokens
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if tokens.as_ref() != Some(expected) {
                    return Ok(false);
                }
                *tokens = None;
                true
            };
            if cleared
                && let Some(on_clear) = self
                    .on_clear
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
            {
                on_clear();
            }
            Ok(cleared)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "clearing"
        }
    }

    #[async_trait]
    impl TokenStore for ReplacingAfterLoadTokenStore {
        async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            let current = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let should_replace = {
                let mut replace_on_next_load = self
                    .replace_on_next_load
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let should_replace = *replace_on_next_load;
                *replace_on_next_load = false;
                should_replace
            };
            if should_replace {
                *self
                    .tokens
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(self.replacement.clone());
                if let Some(on_replace) = self
                    .on_replace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    on_replace();
                }
            }
            Ok(current)
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

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            expected: Option<&PersistedTokens>,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != expected {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            *self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Ok(())
        }

        async fn clear_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = None;
            Ok(true)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "replacing_after_load"
        }
    }

    #[async_trait]
    impl TokenStore for SaveObservingTokenStore {
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
            if let Some(on_save) = self
                .on_save
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                on_save(tokens);
            }
            *self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tokens.clone());
            Ok(())
        }

        async fn save_if_current(
            &self,
            key: &TokenKey,
            expected: &PersistedTokens,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            if self.load(key).await?.as_ref() != Some(expected) {
                return Ok(false);
            }
            self.save(key, replacement).await?;
            Ok(true)
        }

        async fn save_if_current_optional(
            &self,
            key: &TokenKey,
            expected: Option<&PersistedTokens>,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            if self.load(key).await?.as_ref() != expected {
                return Ok(false);
            }
            self.save(key, replacement).await?;
            Ok(true)
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            *self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Ok(())
        }

        async fn clear_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = None;
            Ok(true)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "save_observing"
        }
    }

    #[async_trait]
    impl TokenStore for LoadUnavailableInitialSaveTokenStore {
        async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            Err(TokenStoreError::KeyringUnavailable(
                "keyring unavailable and no file fallback material is present".into(),
            ))
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

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            expected: Option<&PersistedTokens>,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != expected {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            *self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Ok(())
        }

        async fn clear_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = None;
            Ok(true)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "load_unavailable_initial_save"
        }
    }

    #[async_trait]
    impl TokenStore for FinalizeRaceTokenStore {
        async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            if *self
                .fail_load
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                return Err(TokenStoreError::Serde("corrupt token".into()));
            }
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

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            if replacement
                .auth_lease
                .as_ref()
                .and_then(|binding| binding.pending_owner_generation)
                .is_none()
            {
                if self.fail_final_save_after_writing {
                    *tokens = Some(replacement.clone());
                    return Err(TokenStoreError::Unavailable(
                        "finalize write reported failure".into(),
                    ));
                }
                if self.fail_load_after_final_save_attempt {
                    *self
                        .fail_load
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                }
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            expected: Option<&PersistedTokens>,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != expected {
                return Ok(false);
            }
            *tokens = Some(replacement.clone());
            Ok(true)
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            *self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Ok(())
        }

        async fn clear_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut tokens = self
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tokens.as_ref() != Some(expected) {
                return Ok(false);
            }
            *tokens = None;
            Ok(true)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "finalize_race"
        }
    }

    #[test]
    fn persisted_token_expires_at_epoch_secs_uses_persisted_token_expiry() {
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let tokens = tokens_with_expiry(Some(expires_at));

        assert_eq!(
            persisted_token_expires_at_epoch_secs(&tokens),
            1_800_000_000
        );
    }

    #[test]
    fn persisted_token_expires_at_epoch_secs_maps_non_expiring_tokens_to_unbounded_lease() {
        let tokens = tokens_with_expiry(None);

        assert_eq!(persisted_token_expires_at_epoch_secs(&tokens), u64::MAX);
    }

    #[tokio::test]
    async fn save_boundary_persists_pending_tokens_before_publishing_valid_lifecycle() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            generation: 0,
        });
        let observed_handle = Arc::clone(&handle);
        let observed_key = key.clone();
        let store = SaveObservingTokenStore::new(Box::new(move |tokens| {
            assert_eq!(
                observed_handle.snapshot(&lease_key).phase,
                None,
                "TokenStore material must be present before AuthMachine publishes Valid"
            );
            let binding = tokens
                .auth_lease
                .as_ref()
                .expect("pending save must carry an AuthMachine binding");
            assert_eq!(binding.token_key, observed_key);
            assert_eq!(binding.pending_owner_generation, Some(0));
        }));
        let tokens = tokens_with_expiry(None);

        save_tokens_and_publish_lifecycle_acquired(
            &store,
            handle.as_ref(),
            &connection_ref,
            &tokens,
        )
        .await
        .unwrap();

        let snapshot = handle.snapshot(&LeaseKey::from_connection_ref(&connection_ref));
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        let stored = store.load(&key).await.unwrap().unwrap();
        assert!(persisted_tokens_match_lifecycle_snapshot(
            &stored, &key, &snapshot
        ));
        assert_eq!(stored.auth_lease.unwrap().pending_owner_generation, None);
    }

    #[tokio::test]
    async fn save_boundary_uses_initial_cas_when_preload_keyring_is_unavailable() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            generation: 0,
        });
        let store = LoadUnavailableInitialSaveTokenStore::new();
        let tokens = tokens_with_expiry(None);

        save_tokens_and_publish_lifecycle_acquired(
            &store,
            handle.as_ref(),
            &connection_ref,
            &tokens,
        )
        .await
        .unwrap();

        let snapshot = handle.snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        let stored = store.stored().expect("stored finalized tokens");
        assert!(persisted_tokens_match_lifecycle_snapshot(
            &stored, &key, &snapshot
        ));
        assert_eq!(
            stored.auth_lease.unwrap().pending_owner_generation,
            None,
            "preload keyring outage must still finalize through store CAS, not leave pending material"
        );
    }

    #[tokio::test]
    async fn save_boundary_rejects_pending_material_when_final_binding_is_not_persisted() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            generation: 0,
        });
        let store = FinalizeRaceTokenStore::new();
        let tokens = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap(),
        ));

        let err = save_tokens_and_publish_lifecycle_acquired(
            &store,
            handle.as_ref(),
            &connection_ref,
            &tokens,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            TokenLifecycleSaveError::TokenStoreFinalizeRace
        ));
        assert_eq!(
            handle.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired),
            "unfinalized durable material must invalidate the AuthMachine lease"
        );
        let stored = store.load(&key).await.unwrap().unwrap();
        let binding = stored.auth_lease.expect("pending material remains stored");
        assert_eq!(binding.pending_owner_generation, Some(0));
    }

    #[tokio::test]
    async fn save_boundary_marks_reauth_when_final_binding_verification_load_fails() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            generation: 0,
        });
        let store = FinalizeRaceTokenStore::new_with_finalize_load_error();
        let tokens = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap(),
        ));

        let err = save_tokens_and_publish_lifecycle_acquired(
            &store,
            handle.as_ref(),
            &connection_ref,
            &tokens,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            TokenLifecycleSaveError::TokenStoreFinalizeLoad(_)
        ));
        assert_eq!(
            handle.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired),
            "unverifiable durable material must invalidate the AuthMachine lease"
        );
    }

    #[tokio::test]
    async fn save_boundary_marks_reauth_when_final_save_errors_after_bound_material_is_visible() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            generation: 0,
        });
        let store = FinalizeRaceTokenStore::new_with_final_save_error_after_write();
        let tokens = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap(),
        ));

        let err = save_tokens_and_publish_lifecycle_acquired(
            &store,
            handle.as_ref(),
            &connection_ref,
            &tokens,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            TokenLifecycleSaveError::TokenStoreFinalize(_)
        ));
        assert_eq!(
            handle.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired),
            "reported finalization errors must not be forgiven by reloading visible bound material"
        );
        assert_eq!(
            store.load(&key).await.unwrap(),
            None,
            "visible bound material from a reported finalization error must be cleared so restart cannot bootstrap it"
        );
    }

    #[tokio::test]
    async fn save_boundary_does_not_acquire_over_newer_authmachine_truth_after_token_prepare() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            generation: 0,
        });
        let concurrent_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::ReauthRequired),
            expires_at: Some(1_800_000_000),
            generation: 9,
        };
        let expected_concurrent_snapshot = concurrent_snapshot.clone();
        let handle_for_save = Arc::clone(&handle);
        let observed_lease_key = lease_key.clone();
        let store = SaveObservingTokenStore::new(Box::new(move |_tokens| {
            handle_for_save.force_snapshot(concurrent_snapshot.clone());
        }));
        let tokens = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap(),
        ));

        let err = save_tokens_and_publish_lifecycle_acquired(
            &store,
            handle.as_ref(),
            &connection_ref,
            &tokens,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            TokenLifecycleSaveError::AuthMachineAcquireRace {
                pending_token_restored: true,
            }
        ));
        assert!(
            handle.acquired().is_empty(),
            "save boundary must not unconditionally acquire over newer AuthMachine truth"
        );
        assert_eq!(
            handle.snapshot(&observed_lease_key),
            expected_concurrent_snapshot
        );
        assert!(
            store.load(&key).await.unwrap().is_none(),
            "pending token material must be rolled back when AuthMachine truth wins the race"
        );
    }

    #[tokio::test]
    async fn save_boundary_moves_existing_valid_lifecycle_to_refreshing_before_pending_save() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        let previous = tokens_with_expiry(None).with_auth_lease_binding(key.clone(), 1);
        let observed_handle = Arc::clone(&handle);
        let observed_key = key.clone();
        let observed_lease_key = lease_key.clone();
        let store = SaveObservingTokenStore::new_with_initial(
            Some(previous),
            Box::new(move |tokens| {
                assert_eq!(
                    observed_handle.snapshot(&observed_lease_key).phase,
                    Some(AuthLeasePhase::Refreshing),
                    "existing leases must be AuthMachine-owned as Refreshing before pending TokenStore material is visible"
                );
                let binding = tokens
                    .auth_lease
                    .as_ref()
                    .expect("pending save must carry an AuthMachine binding");
                assert_eq!(binding.token_key, observed_key);
                assert_eq!(binding.pending_owner_generation, Some(2));
            }),
        );
        let replacement = PersistedTokens {
            primary_secret: Some("replacement".into()),
            ..tokens_with_expiry(None)
        };

        save_tokens_and_publish_lifecycle_acquired(
            &store,
            handle.as_ref(),
            &connection_ref,
            &replacement,
        )
        .await
        .unwrap();

        let snapshot = handle.snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        let stored = store.load(&key).await.unwrap().unwrap();
        assert!(persisted_tokens_match_lifecycle_snapshot(
            &stored, &key, &snapshot
        ));
        assert_eq!(stored.auth_lease.unwrap().pending_owner_generation, None);
    }

    #[tokio::test]
    async fn clear_boundary_does_not_release_lifecycle_when_token_clear_fails() {
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
            Vec::<LeaseKey>::new(),
            "failed durable clear must leave AuthMachine truth untouched"
        );
        assert!(handle.acquired().is_empty());
        assert!(
            store
                .load(&TokenKey::from_connection_ref(&connection_ref))
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn clear_boundary_does_not_release_or_restore_when_clear_fails() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let expires_at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let tokens = tokens_with_expiry(Some(expires_at));
        let concurrent_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(1_900_000_000),
            generation: 42,
        };
        let expected_concurrent_snapshot = concurrent_snapshot.clone();
        let handle_for_clear = Arc::clone(&handle);
        let store = ClearFailingTokenStore::new_with_on_clear(
            tokens,
            Some(Box::new(move || {
                handle_for_clear.force_snapshot(concurrent_snapshot.clone());
            })),
        );
        let connection_ref = connection_ref();

        let err =
            clear_tokens_and_publish_lifecycle_released(&store, handle.as_ref(), &connection_ref)
                .await
                .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreClear(_)));
        assert_eq!(
            handle.released(),
            Vec::<LeaseKey>::new(),
            "failed durable clear must not release AuthMachine truth"
        );
        assert!(
            handle.acquired().is_empty(),
            "clear failure must not attempt lifecycle restore over concurrent truth"
        );
        assert_eq!(
            handle.snapshot(&LeaseKey::from_connection_ref(&connection_ref)),
            expected_concurrent_snapshot
        );
    }

    #[tokio::test]
    async fn clear_boundary_restores_token_material_when_release_fails_after_clear() {
        let handle = RecordingAuthLeaseHandle::default();
        handle.reject_release();
        let connection_ref = connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(1_800_000_000),
            generation: 7,
        });
        let previous = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap(),
        ))
        .with_auth_lease_binding(key.clone(), 7);
        let store = ClearingTokenStore::new(previous.clone(), None);

        let err = clear_tokens_and_publish_lifecycle_released(&store, &handle, &connection_ref)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            TokenLifecycleClearError::AuthMachineRelease(_)
        ));
        assert_eq!(
            store.load(&key).await.unwrap(),
            Some(previous),
            "release failure after durable clear must restore the cleared token material"
        );
        assert_eq!(
            handle.snapshot(&lease_key),
            AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(1_800_000_000),
                generation: 7,
            }
        );
    }

    #[tokio::test]
    async fn clear_boundary_fails_closed_when_previous_token_load_fails() {
        let handle = RecordingAuthLeaseHandle::default();
        let store = LoadFailingTokenStore::new();
        let connection_ref = connection_ref();

        let err = clear_tokens_and_publish_lifecycle_released(&store, &handle, &connection_ref)
            .await
            .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreLoad(_)));
        assert!(!store.cleared());
        assert!(
            handle.released().is_empty() && handle.acquired().is_empty(),
            "unreadable previous tokens cannot be safely cleared or released"
        );
    }

    #[tokio::test]
    async fn clear_boundary_does_not_release_lifecycle_when_load_fails() {
        let handle = RecordingAuthLeaseHandle::default();
        let store = LoadFailingTokenStore::new_with_clear_error();
        let connection_ref = connection_ref();

        let err = clear_tokens_and_publish_lifecycle_released(&store, &handle, &connection_ref)
            .await
            .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreLoad(_)));
        assert!(!store.cleared());
        assert!(
            handle.released().is_empty(),
            "lifecycle must remain untouched when token material cannot be loaded"
        );
        assert!(
            handle.acquired().is_empty(),
            "unreadable previous tokens cannot be used to restore a lease"
        );
    }

    #[tokio::test]
    async fn clear_boundary_reports_race_when_token_appears_before_release() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        let replacement = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_900_000_000, 0).unwrap(),
        ));
        let concurrent_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(1_900_000_000),
            generation: 42,
        };
        let expected_snapshot = concurrent_snapshot.clone();
        let hook_handle = Arc::clone(&handle);
        let hook_key = lease_key.clone();
        let store = ReplacingAfterLoadTokenStore::new(
            replacement.clone(),
            Some(Box::new(move || {
                hook_handle.force_snapshot(concurrent_snapshot.clone());
            })),
        );

        let err =
            clear_tokens_and_publish_lifecycle_released(&store, handle.as_ref(), &connection_ref)
                .await
                .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreClearRace));
        assert!(
            handle.released().is_empty(),
            "lost conditional release must not report logout success over newer material"
        );
        assert_eq!(handle.snapshot(&hook_key), expected_snapshot);
        assert_eq!(
            store
                .load(&TokenKey::from_connection_ref(&connection_ref))
                .await
                .unwrap(),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn clear_boundary_reports_race_when_lease_appears_without_token_material() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            generation: 0,
        });
        let concurrent_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(1_900_000_000),
            generation: 42,
        };
        let expected_snapshot = concurrent_snapshot.clone();
        let hook_handle = Arc::clone(&handle);
        let store = EmptyTokenStore::new_with_on_load(Some(Box::new(move || {
            hook_handle.force_snapshot(concurrent_snapshot.clone());
        })));

        let err =
            clear_tokens_and_publish_lifecycle_released(&store, handle.as_ref(), &connection_ref)
                .await
                .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreClearRace));
        assert_eq!(handle.snapshot(&lease_key), expected_snapshot);
    }

    #[tokio::test]
    async fn unreadable_clear_boundary_releases_after_malformed_material_is_cleared() {
        let handle = RecordingAuthLeaseHandle::default();
        let store = UnreadableClearingTokenStore::new();
        let connection_ref = connection_ref();

        let cleared = clear_unreadable_tokens_and_publish_lifecycle_released(
            &store,
            &handle,
            &connection_ref,
        )
        .await
        .unwrap();

        assert!(cleared);
        assert!(store.cleared());
        assert_eq!(
            handle.released(),
            vec![LeaseKey::from_connection_ref(&connection_ref)]
        );
    }

    #[tokio::test]
    async fn unreadable_clear_boundary_does_not_release_newer_lifecycle_after_clear() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let concurrent_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(1_900_000_000),
            generation: 42,
        };
        let expected_snapshot = concurrent_snapshot.clone();
        let handle_for_clear = Arc::clone(&handle);
        let store = UnreadableClearingTokenStore::new_with_on_clear(Some(Box::new(move || {
            handle_for_clear.force_snapshot(concurrent_snapshot.clone());
        })));

        let err = clear_unreadable_tokens_and_publish_lifecycle_released(
            &store,
            handle.as_ref(),
            &connection_ref,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreClearRace));
        assert!(store.cleared());
        assert!(
            handle.released().is_empty(),
            "malformed-token cleanup must not release a lease reacquired after the clear began"
        );
        assert_eq!(
            handle.snapshot(&LeaseKey::from_connection_ref(&connection_ref)),
            expected_snapshot
        );
    }

    #[tokio::test]
    async fn unreadable_clear_boundary_reports_race_when_token_appears_after_clear() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let concurrent_tokens = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_900_000_000, 0).unwrap(),
        ))
        .with_auth_lease_binding(key.clone(), 42);
        let concurrent_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(1_900_000_000),
            generation: 42,
        };
        let handle_for_clear = Arc::clone(&handle);
        let store = UnreadableClearingTokenStore::new_with_on_clear_and_tokens_after_clear(
            Some(Box::new(move || {
                handle_for_clear.force_snapshot(concurrent_snapshot.clone());
            })),
            Some(concurrent_tokens.clone()),
        );

        let err = clear_unreadable_tokens_and_publish_lifecycle_released(
            &store,
            handle.as_ref(),
            &connection_ref,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreClearRace));
        assert_eq!(store.load(&key).await.unwrap(), Some(concurrent_tokens));
    }

    #[tokio::test]
    async fn clear_boundary_does_not_release_lifecycle_when_token_was_replaced() {
        let handle = RecordingAuthLeaseHandle::default();
        let previous = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap(),
        ));
        let replacement = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_900_000_000, 0).unwrap(),
        ));
        let store = ReplacingOnClearTokenStore::new(previous, replacement.clone());
        let connection_ref = connection_ref();

        let err = clear_tokens_and_publish_lifecycle_released(&store, &handle, &connection_ref)
            .await
            .unwrap_err();

        assert!(
            matches!(err, TokenLifecycleClearError::TokenStoreClearRace),
            "conditional clear races must fail closed instead of reporting logout success"
        );
        assert!(
            handle.released().is_empty(),
            "logout must not release AuthMachine truth when newer durable material remains"
        );
        let stored = store
            .load(&TokenKey::from_connection_ref(&connection_ref))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.expires_at, replacement.expires_at,
            "logout must not clear token material that no longer matches the loaded snapshot"
        );
    }

    #[tokio::test]
    async fn clear_boundary_releases_current_lifecycle_after_transient_phase_change() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let token_key = TokenKey::from_connection_ref(&connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(1_800_000_000),
            generation: 7,
        });
        let previous = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap(),
        ))
        .with_auth_lease_binding(token_key.clone(), 7);
        let hook_handle = Arc::clone(&handle);
        let hook_key = lease_key.clone();
        let store = ClearingTokenStore::new(
            previous,
            Some(Box::new(move || {
                hook_handle.mark_expiring(&hook_key).unwrap();
            })),
        );

        clear_tokens_and_publish_lifecycle_released(&store, handle.as_ref(), &connection_ref)
            .await
            .unwrap();

        assert_eq!(handle.released(), vec![lease_key]);
        assert!(
            store.load(&token_key).await.unwrap().is_none(),
            "the old durable material must remain cleared"
        );
    }

    #[tokio::test]
    async fn clear_boundary_restores_tokens_when_fallback_release_loses_to_refreshing_owner() {
        let handle = Arc::new(RecordingAuthLeaseHandle::default());
        let connection_ref = connection_ref();
        let token_key = TokenKey::from_connection_ref(&connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        handle.force_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(1_800_000_000),
            generation: 7,
        });
        let previous = tokens_with_expiry(Some(
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap(),
        ))
        .with_auth_lease_binding(token_key.clone(), 7);
        let hook_handle = Arc::clone(&handle);
        handle.set_conditional_release_hook(Arc::new(move |key, expected| {
            if expected.phase == Some(AuthLeasePhase::Expiring) {
                hook_handle.begin_refresh(key).unwrap();
            }
        }));
        let clear_hook_handle = Arc::clone(&handle);
        let clear_hook_key = lease_key.clone();
        let store = ClearingTokenStore::new(
            previous.clone(),
            Some(Box::new(move || {
                clear_hook_handle.mark_expiring(&clear_hook_key).unwrap();
            })),
        );

        let err =
            clear_tokens_and_publish_lifecycle_released(&store, handle.as_ref(), &connection_ref)
                .await
                .unwrap_err();

        assert!(matches!(err, TokenLifecycleClearError::TokenStoreClearRace));
        assert_eq!(
            store.load(&token_key).await.unwrap(),
            Some(previous),
            "old material must be restored when a new refresh owner still depends on it"
        );
        let snapshot = handle.snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Refreshing));
        assert_eq!(snapshot.expires_at, Some(1_800_000_000));
    }

    #[test]
    fn published_status_projects_lease_phase_without_token_material() {
        let now = Utc::now();
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some((now + chrono::Duration::hours(1)).timestamp() as u64),
            generation: 1,
        };

        let key = TokenKey::parse("dev", "default_openai").unwrap();

        let status = project_published_auth_status(now, &key, None, &snapshot);

        assert_eq!(status.phase, AuthStatusPhase::Valid);
        assert!(status.expires_at.is_some());
        assert!(status.tokens.is_none());
    }

    #[test]
    fn published_status_rejects_stale_token_store_material() {
        let now = Utc::now();
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: None,
            generation: 7,
        };
        let key = TokenKey::parse("dev", "default_openai").unwrap();
        let stale = tokens_with_expiry(Some(now + chrono::Duration::hours(1)));

        let status = project_published_auth_status(now, &key, Some(&stale), &snapshot);

        assert_eq!(status.phase, AuthStatusPhase::Valid);
        assert!(status.expires_at.is_none());
        assert!(status.tokens.is_none());
    }
}
