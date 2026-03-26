use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::BoxFuture;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::{Session, SessionId};

#[cfg(target_arch = "wasm32")]
use crate::tokio;
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::task::JoinHandle;

/// Lock a std::sync::Mutex, recovering from poisoning.
///
/// These mutexes guard small synchronous data swaps (no async work under lock).
/// Poisoning can only occur if a thread panicked mid-swap, which should never
/// happen in practice. We recover by accepting the inner data rather than
/// propagating the panic.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub type RequestAsyncAction = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

pub fn request_action<F, Fut>(f: F) -> RequestAsyncAction
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Arc::new(move || Box::pin(f()))
}

pub fn noop_request_action() -> RequestAsyncAction {
    request_action(|| async {})
}

/// Returned by [`SurfaceRequestExecutor::try_begin_request`] when the key
/// is already tracked as an in-flight request.
#[derive(Debug)]
pub struct RequestAlreadyExists;

#[derive(Debug)]
pub enum RequestTerminal<T> {
    Publish(T),
    RespondWithoutPublish(T),
}

struct RequestEntry {
    cancel_requested: AtomicBool,
    ownership_published: AtomicBool,
    cancel_action: Mutex<RequestAsyncAction>,
    unpublished_cleanup: Mutex<Option<RequestAsyncAction>>,
    task_handle: Mutex<Option<JoinHandle<()>>>,
}

impl RequestEntry {
    fn new(initial_cancel: RequestAsyncAction) -> Self {
        Self {
            cancel_requested: AtomicBool::new(false),
            ownership_published: AtomicBool::new(false),
            cancel_action: Mutex::new(initial_cancel),
            unpublished_cleanup: Mutex::new(None),
            task_handle: Mutex::new(None),
        }
    }
}

#[derive(Clone)]
pub struct RequestContext {
    key: String,
    entry: Arc<RequestEntry>,
}

impl RequestContext {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn replace_cancel_action(&self, action: RequestAsyncAction) {
        let mut slot = self.cancel_action_guard();
        *slot = action;
    }

    pub fn set_unpublished_cleanup(&self, cleanup: RequestAsyncAction) {
        let mut slot = self.cleanup_guard();
        *slot = Some(cleanup);
    }

    pub fn clear_unpublished_cleanup(&self) {
        let mut slot = self.cleanup_guard();
        *slot = None;
    }

    pub fn cancel_requested(&self) -> bool {
        self.entry.cancel_requested.load(Ordering::Acquire)
    }

    pub async fn run_cancel_if_requested(&self) -> bool {
        if !self.cancel_requested() {
            return false;
        }

        let action = {
            let slot = self.cancel_action_guard();
            Arc::clone(&slot)
        };
        action().await;
        true
    }

    fn cancel_action_guard(&self) -> MutexGuard<'_, RequestAsyncAction> {
        lock_or_recover(&self.entry.cancel_action)
    }

    fn cleanup_guard(&self) -> MutexGuard<'_, Option<RequestAsyncAction>> {
        lock_or_recover(&self.entry.unpublished_cleanup)
    }
}

#[derive(Clone)]
pub struct SurfaceRequestExecutor {
    entries: Arc<Mutex<HashMap<String, Arc<RequestEntry>>>>,
    shutdown_grace: Duration,
}

impl SurfaceRequestExecutor {
    pub fn new(shutdown_grace: Duration) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            shutdown_grace,
        }
    }

    pub fn begin_request(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
    ) -> RequestContext {
        let key = key.into();
        let entry = Arc::new(RequestEntry::new(initial_cancel));
        let mut entries = lock_or_recover(&self.entries);
        entries.insert(key.clone(), Arc::clone(&entry));
        RequestContext { key, entry }
    }

    /// Fallible variant of `begin_request` that rejects duplicate in-flight keys.
    ///
    /// Returns `Err(RequestAlreadyExists)` if a request with the same key is
    /// already tracked. This prevents REST callers from silently overwriting an
    /// in-flight request's cancel/cleanup state.
    pub fn try_begin_request(
        &self,
        key: impl Into<String>,
        initial_cancel: RequestAsyncAction,
    ) -> Result<RequestContext, RequestAlreadyExists> {
        let key = key.into();
        let mut entries = lock_or_recover(&self.entries);
        if entries.contains_key(&key) {
            return Err(RequestAlreadyExists);
        }
        let entry = Arc::new(RequestEntry::new(initial_cancel));
        entries.insert(key.clone(), Arc::clone(&entry));
        Ok(RequestContext { key, entry })
    }

    pub fn attach_task(&self, key: &str, handle: JoinHandle<()>) {
        if let Some(entry) = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
        {
            let mut slot = entry
                .task_handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *slot = Some(handle);
        }
    }

    pub fn cancel_requested(&self, key: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .is_some_and(|entry| entry.cancel_requested.load(Ordering::Acquire))
    }

    pub async fn cancel_request(&self, key: &str) -> bool {
        let Some(entry) = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
        else {
            return false;
        };

        entry.cancel_requested.store(true, Ordering::Release);
        if entry.ownership_published.load(Ordering::Acquire) {
            return true;
        }

        let action = {
            let slot = entry
                .cancel_action
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(&slot)
        };
        action().await;
        true
    }

    pub fn mark_published(&self, key: &str) {
        if let Some(entry) = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
        {
            entry.ownership_published.store(true, Ordering::Release);
        }
    }

    pub fn remove_published(&self, key: &str) {
        let mut entries = lock_or_recover(&self.entries);
        entries.remove(key);
    }

    pub async fn finish_unpublished(&self, key: &str) {
        let entry = {
            let mut entries = lock_or_recover(&self.entries);
            entries.remove(key)
        };

        let Some(entry) = entry else {
            return;
        };

        if entry.ownership_published.load(Ordering::Acquire) {
            return;
        }

        let cleanup = entry
            .unpublished_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(cleanup) = cleanup {
            cleanup().await;
        }
    }

    pub async fn cancel_all(&self) {
        let keys: Vec<String> = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();

        for key in keys {
            let _ = self.cancel_request(&key).await;
        }
    }

    pub async fn shutdown_and_abort_stragglers(&self) {
        let has_entries = !self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if has_entries {
            self.cancel_all().await;
            tokio::time::sleep(self.shutdown_grace).await;
        }

        let remaining: Vec<(String, Arc<RequestEntry>)> = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(key, entry)| (key.clone(), Arc::clone(entry)))
            .collect();

        for (key, entry) in remaining {
            if let Some(handle) = lock_or_recover(&entry.task_handle).take() {
                handle.abort();
            }
            if !entry.ownership_published.load(Ordering::Acquire) {
                let cleanup = entry
                    .unpublished_cleanup
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(cleanup) = cleanup {
                    cleanup().await;
                }
            }
            let mut entries = lock_or_recover(&self.entries);
            entries.remove(&key);
        }
    }
}

pub struct PreparedSurfaceSession {
    pub session: Session,
    pub session_id: SessionId,
    pub ops_lifecycle: Arc<dyn OpsLifecycleRegistry>,
}

pub async fn prepare_surface_session(
    runtime_adapter: &meerkat_runtime::RuntimeSessionAdapter,
) -> Result<PreparedSurfaceSession, String> {
    let session = Session::new();
    let session_id = session.id().clone();
    runtime_adapter.register_session(session_id.clone()).await;
    let ops_lifecycle = runtime_adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .map(|registry| registry as Arc<dyn OpsLifecycleRegistry>)
        .ok_or_else(|| format!("failed to obtain runtime ops registry for session {session_id}"))?;
    Ok(PreparedSurfaceSession {
        session,
        session_id,
        ops_lifecycle,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn finish_unpublished_runs_cleanup_once() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request("req-1", noop_request_action());
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        executor.finish_unpublished("req-1").await;
        executor.finish_unpublished("req-1").await;

        assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn published_request_skips_unpublished_cleanup() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request("req-2", noop_request_action());
        context.set_unpublished_cleanup(request_action({
            let cleanup_count = Arc::clone(&cleanup_count);
            move || {
                let cleanup_count = Arc::clone(&cleanup_count);
                async move {
                    cleanup_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        executor.mark_published("req-2");
        executor.finish_unpublished("req-2").await;

        assert_eq!(cleanup_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_uses_latest_installed_action() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let initial_count = Arc::new(AtomicUsize::new(0));
        let upgraded_count = Arc::new(AtomicUsize::new(0));
        let context = executor.begin_request(
            "req-3",
            request_action({
                let initial_count = Arc::clone(&initial_count);
                move || {
                    let initial_count = Arc::clone(&initial_count);
                    async move {
                        initial_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }),
        );

        context.replace_cancel_action(request_action({
            let upgraded_count = Arc::clone(&upgraded_count);
            move || {
                let upgraded_count = Arc::clone(&upgraded_count);
                async move {
                    upgraded_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        assert!(executor.cancel_request("req-3").await);
        assert_eq!(initial_count.load(Ordering::SeqCst), 0);
        assert_eq!(upgraded_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn try_begin_request_rejects_duplicate_key() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let _ctx = executor
            .try_begin_request("dup-key", noop_request_action())
            .expect("first registration should succeed");
        let result = executor.try_begin_request("dup-key", noop_request_action());
        assert!(result.is_err(), "duplicate key should be rejected");
    }

    #[tokio::test]
    async fn try_begin_request_allows_after_removal() {
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let _ctx = executor
            .try_begin_request("reuse-key", noop_request_action())
            .expect("first registration should succeed");
        executor.finish_unpublished("reuse-key").await;
        let result = executor.try_begin_request("reuse-key", noop_request_action());
        assert!(
            result.is_ok(),
            "key should be available after previous request is removed"
        );
    }
}
