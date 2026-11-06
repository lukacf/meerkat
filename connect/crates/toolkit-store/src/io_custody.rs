//! Carry an existing caller's lifetime through detached store I/O.
//!
//! This scope is not authorization, a transaction, or an outcome. It only
//! keeps opaque owner values alive until the actual backend call finishes.
use std::{future::Future, sync::Arc};

tokio::task_local! {
    static IO_CUSTODY: Vec<Arc<dyn Send + Sync>>;
}

/// Retain an existing owner while this future runs and while SQLite calls
/// entered by it finish, even if the future is cancelled. Nested scopes keep
/// their parents. Spawned tasks must explicitly establish their own scope;
/// this does not propagate authority or scope to unrelated tasks.
pub async fn retain_io_custody<F: Future>(custody: Arc<dyn Send + Sync>, future: F) -> F::Output {
    let mut retained = capture();
    retained.push(custody);
    IO_CUSTODY.scope(retained, future).await
}

pub(crate) fn capture() -> Vec<Arc<dyn Send + Sync>> {
    IO_CUSTODY.try_with(Clone::clone).unwrap_or_default()
}
