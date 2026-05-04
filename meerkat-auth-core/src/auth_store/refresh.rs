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

use super::{PersistedTokens, RefreshCoordinator, RefreshError, RefreshFn, TokenKey};

// ---------------------------------------------------------------------
// InMemoryCoordinator
// ---------------------------------------------------------------------

type SharedRefresh = Shared<BoxFuture<'static, Result<PersistedTokens, RefreshError>>>;

/// In-process refresh dedup. All refreshes for the same key coalesce into
/// a single underlying future; subsequent callers observe the same result
/// via `Shared`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum RefreshIntent {
    Normal,
    Forced,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct InFlightRefreshKey {
    token: TokenKey,
    intent: RefreshIntent,
}

#[derive(Clone, Default)]
pub struct InMemoryCoordinator {
    in_flight: Arc<Mutex<HashMap<InFlightRefreshKey, SharedRefresh>>>,
}

impl InMemoryCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    async fn with_refresh_intent(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
        intent: RefreshIntent,
    ) -> Result<PersistedTokens, RefreshError> {
        let in_flight_key = InFlightRefreshKey { token: key, intent };
        let fut = {
            let mut map = self.in_flight.lock();
            if let Some(existing) = map.get(&in_flight_key) {
                existing.clone()
            } else {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let shared: SharedRefresh =
                    async move { rx.await.unwrap_or(Err(RefreshError::Cancelled)) }
                        .boxed()
                        .shared();
                map.insert(in_flight_key.clone(), shared.clone());
                let in_flight = Arc::clone(&self.in_flight);
                let cleanup_key = in_flight_key.clone();
                tokio::spawn(async move {
                    let result = refresh_fn().await;
                    let _ = tx.send(result);
                    in_flight.lock().remove(&cleanup_key);
                });
                shared
            }
        };
        let result = fut.await;
        // Remove from the in-flight map so subsequent refreshes are not
        // short-circuited by the terminal result.
        self.in_flight.lock().remove(&in_flight_key);
        result
    }
}

#[async_trait]
impl RefreshCoordinator for InMemoryCoordinator {
    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        self.with_refresh_intent(key, refresh_fn, RefreshIntent::Normal)
            .await
    }

    async fn with_forced_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        self.with_refresh_intent(key, refresh_fn, RefreshIntent::Forced)
            .await
    }
}

// ---------------------------------------------------------------------
// FileLockCoordinator (cross-process dedup)
// ---------------------------------------------------------------------

#[cfg(feature = "file-lock")]
pub use file_lock::FileLockCoordinator;

#[cfg(feature = "file-lock")]
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
                .join(format!("{}--{}.lock", key.realm, key.binding))
        }

        fn with_locking_refresh(&self, key: &TokenKey, refresh_fn: RefreshFn) -> RefreshFn {
            let lock_dir = self.lock_dir.clone();
            let lock_path = self.lock_path_for(key);
            Box::new(move || {
                Box::pin(async move {
                    tokio::fs::create_dir_all(&lock_dir)
                        .await
                        .map_err(|e| RefreshError::LockFailed(e.to_string()))?;

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

                    let result = refresh_fn().await;

                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = FileExt::unlock(&file);
                        drop(file);
                    })
                    .await;

                    result
                })
            })
        }
    }

    #[async_trait]
    impl RefreshCoordinator for FileLockCoordinator {
        async fn with_refresh(
            &self,
            key: TokenKey,
            refresh_fn: RefreshFn,
        ) -> Result<PersistedTokens, RefreshError> {
            let refresh_fn = self.with_locking_refresh(&key, refresh_fn);
            self.inner.with_refresh(key, refresh_fn).await
        }

        async fn with_forced_refresh(
            &self,
            key: TokenKey,
            refresh_fn: RefreshFn,
        ) -> Result<PersistedTokens, RefreshError> {
            let refresh_fn = self.with_locking_refresh(&key, refresh_fn);
            self.inner.with_forced_refresh(key, refresh_fn).await
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Utc;
    use meerkat_core::{BindingId, RealmId};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::oneshot;

    fn key() -> TokenKey {
        TokenKey::new(
            RealmId::parse("dev").expect("valid realm"),
            BindingId::parse("default_openai").expect("valid binding"),
        )
    }

    fn tokens(access_token: &str) -> PersistedTokens {
        PersistedTokens {
            auth_mode: super::super::PersistedAuthMode::ChatgptOauth,
            primary_secret: Some(access_token.to_string()),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            expires_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            last_refresh: Some(Utc::now()),
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[tokio::test]
    async fn forced_refresh_does_not_join_normal_in_flight_refresh() {
        let coordinator = InMemoryCoordinator::new();
        let key = key();
        let (normal_started_tx, normal_started_rx) = oneshot::channel();
        let (normal_release_tx, normal_release_rx) = oneshot::channel();
        let normal = {
            let coordinator = coordinator.clone();
            let key = key.clone();
            tokio::spawn(async move {
                coordinator
                    .with_refresh(
                        key,
                        Box::new(move || {
                            Box::pin(async move {
                                let _ = normal_started_tx.send(());
                                normal_release_rx
                                    .await
                                    .map_err(|err| RefreshError::Refresh(err.to_string()))?;
                                Ok(tokens("normal"))
                            })
                        }),
                    )
                    .await
            })
        };
        normal_started_rx.await.expect("normal refresh started");

        let forced = coordinator
            .with_forced_refresh(key, Box::new(|| Box::pin(async { Ok(tokens("forced")) })))
            .await
            .expect("forced refresh should run its own refresh closure");
        assert_eq!(forced.primary_secret.as_deref(), Some("forced"));

        normal_release_tx
            .send(())
            .expect("normal refresh still waiting separately");
        let normal = normal
            .await
            .expect("normal task joins")
            .expect("normal refresh succeeds");
        assert_eq!(normal.primary_secret.as_deref(), Some("normal"));
    }

    #[tokio::test]
    async fn refresh_work_continues_after_origin_waiter_is_cancelled() {
        let coordinator = InMemoryCoordinator::new();
        let key = key();
        let completed = Arc::new(AtomicUsize::new(0));
        let completed_for_refresh = Arc::clone(&completed);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();

        let refresh = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                coordinator
                    .with_refresh(
                        key,
                        Box::new(move || {
                            Box::pin(async move {
                                let _ = started_tx.send(());
                                release_rx
                                    .await
                                    .map_err(|err| RefreshError::Refresh(err.to_string()))?;
                                completed_for_refresh.fetch_add(1, Ordering::SeqCst);
                                Ok(tokens("completed"))
                            })
                        }),
                    )
                    .await
            })
        };

        started_rx.await.expect("refresh closure started");
        refresh.abort();
        release_tx
            .send(())
            .expect("background refresh closure is still retained");

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while completed.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("refresh work should finish after the origin waiter is cancelled");
    }
}
