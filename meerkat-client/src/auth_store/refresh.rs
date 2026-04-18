//! Refresh coordination — in-process dedup + cross-process lockfile.
//!
//! Reference-CLI parity: Codex `manager.rs:1569-1703` (proactive refresh,
//! guarded reload, failure cache), Claude Code `utils/auth.ts:1313-1560`
//! (filesystem lock + in-process dedup via `pending401Handlers`).
//!
//! `InMemoryCoordinator` coalesces concurrent refresh calls for the same
//! `TokenKey` via a shared future — five parallel resolves trigger exactly
//! one `refresh_fn` call; subsequent callers await its result.
//!
//! `FileLockCoordinator` (feature `refresh-file-lock`) wraps the in-memory
//! coordinator with an OS-level lockfile so refreshes are serialized across
//! processes too. Uses the `fs4` crate.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::{BoxFuture, FutureExt, Shared};
use parking_lot::Mutex;
use thiserror::Error;

use super::{PersistedTokens, TokenKey};

/// Errors raised by the refresh coordinator.
#[derive(Clone, Debug, Error)]
pub enum RefreshError {
    #[error("refresh function failed: {0}")]
    Refresh(String),
    #[error("refresh in progress was cancelled")]
    Cancelled,
    #[error("cross-process lock acquisition failed: {0}")]
    LockFailed(String),
}

/// Boxed refresh closure. Takes no args (the caller binds the refresh
/// context via closure capture) and resolves to a token bundle.
pub type RefreshFn =
    Box<dyn FnOnce() -> BoxFuture<'static, Result<PersistedTokens, RefreshError>> + Send + 'static>;

/// Coordinator for token-refresh calls. Implementations must coalesce
/// concurrent calls for the same `TokenKey` so only one underlying refresh
/// runs at a time.
#[async_trait]
pub trait RefreshCoordinator: Send + Sync {
    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError>;
}

// ---------------------------------------------------------------------
// InMemoryCoordinator
// ---------------------------------------------------------------------

type SharedRefresh = Shared<BoxFuture<'static, Result<PersistedTokens, RefreshError>>>;

/// In-process refresh dedup. All refreshes for the same key coalesce into
/// a single underlying future; subsequent callers observe the same result
/// via `Shared`.
#[derive(Clone, Default)]
pub struct InMemoryCoordinator {
    in_flight: Arc<Mutex<HashMap<TokenKey, SharedRefresh>>>,
}

impl InMemoryCoordinator {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RefreshCoordinator for InMemoryCoordinator {
    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        let fut = {
            let mut map = self.in_flight.lock();
            if let Some(existing) = map.get(&key) {
                existing.clone()
            } else {
                let f: BoxFuture<'static, _> = refresh_fn().boxed();
                let shared = f.shared();
                map.insert(key.clone(), shared.clone());
                shared
            }
        };
        let result = fut.await;
        // Remove from the in-flight map so subsequent refreshes are not
        // short-circuited by the terminal result.
        self.in_flight.lock().remove(&key);
        result
    }
}

// ---------------------------------------------------------------------
// FileLockCoordinator (cross-process dedup)
// ---------------------------------------------------------------------

#[cfg(feature = "refresh-file-lock")]
pub use file_lock::FileLockCoordinator;

#[cfg(feature = "refresh-file-lock")]
mod file_lock {
    use std::fs::{File, OpenOptions};
    use std::path::PathBuf;

    use async_trait::async_trait;
    use fs4::fs_std::FileExt;

    use super::{InMemoryCoordinator, RefreshCoordinator, RefreshError, RefreshFn};
    use crate::auth_store::{PersistedTokens, TokenKey};

    /// Wraps `InMemoryCoordinator` with an OS-level lockfile per binding.
    /// Only one process refreshes at a time per binding; in-process dedup
    /// prevents redundant work within a single process.
    ///
    /// Lock acquisition/release is blocking (`fs4::fs_std::FileExt`). We
    /// move the blocking work onto `tokio::task::spawn_blocking`.
    pub struct FileLockCoordinator {
        lock_dir: PathBuf,
        inner: InMemoryCoordinator,
    }

    impl FileLockCoordinator {
        pub fn new(lock_dir: impl Into<PathBuf>) -> Self {
            Self {
                lock_dir: lock_dir.into(),
                inner: InMemoryCoordinator::new(),
            }
        }

        fn lock_path_for(&self, key: &TokenKey) -> PathBuf {
            self.lock_dir
                .join(format!("{}--{}.lock", key.realm_id, key.binding_id))
        }
    }

    #[async_trait]
    impl RefreshCoordinator for FileLockCoordinator {
        async fn with_refresh(
            &self,
            key: TokenKey,
            refresh_fn: RefreshFn,
        ) -> Result<PersistedTokens, RefreshError> {
            tokio::fs::create_dir_all(&self.lock_dir)
                .await
                .map_err(|e| RefreshError::LockFailed(e.to_string()))?;
            let lock_path = self.lock_path_for(&key);

            // Acquire the OS-level exclusive lock off-thread.
            let file = tokio::task::spawn_blocking(move || -> std::io::Result<File> {
                let f = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .open(&lock_path)?;
                f.lock_exclusive()?;
                Ok(f)
            })
            .await
            .map_err(|e| RefreshError::LockFailed(format!("spawn_blocking: {e}")))?
            .map_err(|e| RefreshError::LockFailed(e.to_string()))?;

            let result = self.inner.with_refresh(key, refresh_fn).await;

            // Release lock on a blocking thread, file is dropped (and the
            // kernel releases the lock) at end of this scope.
            let _ = tokio::task::spawn_blocking(move || {
                let _ = FileExt::unlock(&file);
                drop(file);
            })
            .await;

            result
        }
    }
}
