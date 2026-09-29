//! Task spawning for the wasm32 `tokio` shim ([`crate::tokio::task`]).
//!
//! `tokio_with_wasm::spawn` wraps the future it is handed in a select against
//! its cancellation channel and drives that from a `spawn_local` task. The
//! wrapper is monomorphized per future type and moves the future by value
//! while it builds and first polls that select, so each instance's
//! shadow-stack frame is about the size of the future it wraps. At opt-level
//! "s" the largest of 464 instances was 125,440 bytes, and a turn's
//! shadow-stack high-water was 230,404 bytes (issue #1230).
//!
//! Every Meerkat spawn therefore hands the wrapper a boxed trait object: an
//! instance's frame is then the size of a fat pointer and its own locals, the
//! future's state lives on the heap (where `spawn_local` keeps the task
//! anyway), and the wrapper is instantiated once per output type rather than
//! once per future: 70 instances, the largest 4,512 bytes, and a turn's
//! high-water of 115,892 bytes. [`JoinSet::spawn`] does the same for task
//! sets.
//! `sdks/web/scripts/wasm-frames.mjs` holds the release build to a budget for
//! the largest wrapper frame.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio_with_wasm::alias::task::{AbortHandle, JoinError, JoinHandle};

/// The one future type Meerkat hands `tokio_with_wasm`'s spawn wrapper.
type BoxedTask<T> = Pin<Box<dyn Future<Output = T> + 'static>>;

fn boxed<F>(future: F) -> BoxedTask<F::Output>
where
    F: Future + 'static,
{
    Box::pin(future)
}

/// Spawns `future` on the JavaScript event loop, boxed first (see the module
/// documentation). Behaves as `tokio_with_wasm::spawn`.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    tokio_with_wasm::alias::task::spawn(boxed(future))
}

/// `tokio_with_wasm`'s `JoinSet`, whose [`spawn`](JoinSet::spawn) boxes the
/// future first (see the module documentation).
pub struct JoinSet<T> {
    inner: tokio_with_wasm::alias::task::JoinSet<T>,
}

impl<T> JoinSet<T> {
    /// An empty set.
    pub fn new() -> Self {
        Self {
            inner: tokio_with_wasm::alias::task::JoinSet::new(),
        }
    }

    /// The number of tasks in the set.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the set has no tasks.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<T: 'static> JoinSet<T> {
    /// Spawns `task` into the set, boxed first.
    pub fn spawn<F>(&mut self, task: F) -> AbortHandle
    where
        F: Future<Output = T> + 'static,
    {
        self.inner.spawn(boxed(task))
    }

    /// Waits for one of the tasks to complete; `None` when the set is empty.
    pub async fn join_next(&mut self) -> Option<Result<T, JoinError>> {
        self.inner.join_next().await
    }

    /// A completed task's output, without waiting.
    pub fn try_join_next(&mut self) -> Option<Result<T, JoinError>> {
        self.inner.try_join_next()
    }

    /// Polls for one of the tasks to complete.
    pub fn poll_join_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<T, JoinError>>> {
        self.inner.poll_join_next(cx)
    }

    /// Aborts every task and waits for them all to finish.
    pub async fn shutdown(&mut self) {
        self.inner.shutdown().await;
    }

    /// Waits for every task and returns the outputs of those that completed.
    pub async fn join_all(self) -> Vec<T> {
        self.inner.join_all().await
    }

    /// Aborts every task in the set.
    pub fn abort_all(&mut self) {
        self.inner.abort_all();
    }

    /// Removes every task from the set without aborting it.
    pub fn detach_all(&mut self) {
        self.inner.detach_all();
    }
}

impl<T> Default for JoinSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for JoinSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}
