//! Fresh-task stack relief for deep async construction chains.
//!
//! At opt-level=0 LLVM performs no stack-slot coloring, so the poll function
//! of a large async fn reserves a separate stack slot for every local in
//! every branch. Deep construction chains (session runtime registration,
//! agent build, mob machine commands) therefore carry enormous poll frames,
//! and awaiting them inline stacks those frames beneath the caller's own
//! poll chain — e.g. an agent's run loop → tool dispatch → mob spawn →
//! child-session registration. That sum is what overflowed the 2 MiB
//! production worker-stack budget asserted by
//! `tools_full_with_explicit_auth_binding_can_spawn_within_production_stack_budget`.
//!
//! [`relieve_caller_stack`] moves such a chain onto its own tokio task, so
//! its frames start near the top of a fresh task poll rather than on top of
//! the caller's. It takes a future-*maker* rather than a future because
//! `tokio::spawn` moves its argument by value: a large future would
//! otherwise transit the caller's stack (and the spawn call's frame) at its
//! full size. The maker closure is small (its captures), and the future it
//! makes is materialized and boxed on the fresh task's stack instead.

use std::future::Future;

/// Runs the future produced by `make_future` on its own tokio task and
/// awaits its completion, aborting the task if the caller is dropped first.
///
/// Semantics relative to an inline `make_future().await`:
/// - completion and output are identical;
/// - dropping the caller aborts the spawned task at its next await point
///   (inline: the future is dropped at the same point);
/// - a panic inside the future is resumed on the caller.
#[cfg(not(target_arch = "wasm32"))]
pub async fn relieve_caller_stack<T, F, Fut>(make_future: F) -> T
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    /// Aborts the spawned task when the caller's future is dropped
    /// mid-await. Aborting an already-finished task is a no-op, so the
    /// guard is safe to hold across the successful path too.
    struct AbortOnDrop(tokio::task::AbortHandle);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    let handle = tokio::spawn(async move {
        // Materialize the (potentially large) future on this fresh task's
        // stack — not the caller's — and box it so this wrapper's own
        // generator stays at closure-capture size.
        let future: std::pin::Pin<Box<Fut>> = Box::pin(make_future());
        future.await
    });
    let _guard = AbortOnDrop(handle.abort_handle());
    match handle.await {
        Ok(value) => value,
        Err(join_error) => match join_error.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            // Cancellation is only possible via the guard above (not yet
            // dropped) or runtime shutdown. Under shutdown the caller is
            // being torn down as well; parking mirrors the inline-await
            // behavior of a future that will never be polled to completion.
            Err(_) => std::future::pending().await,
        },
    }
}

/// wasm32: single-threaded, no worker-stack budget to defend — await inline.
#[cfg(target_arch = "wasm32")]
pub async fn relieve_caller_stack<T, F, Fut>(make_future: F) -> T
where
    F: FnOnce() -> Fut + 'static,
    Fut: Future<Output = T> + 'static,
    T: 'static,
{
    make_future().await
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::relieve_caller_stack;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn resolves_with_the_future_output() {
        let value = relieve_caller_stack(|| async { 6 * 7 }).await;
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn propagates_panics_to_the_caller() {
        let result = tokio::spawn(async {
            relieve_caller_stack(|| async { panic!("stack relief panic probe") }).await
        })
        .await;
        let join_error = result.expect_err("panic must propagate");
        assert!(join_error.is_panic());
    }

    #[tokio::test]
    async fn dropping_the_caller_aborts_the_spawned_work() {
        let entered = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let entered_clone = Arc::clone(&entered);
        let finished_clone = Arc::clone(&finished);
        let caller = tokio::spawn(async move {
            relieve_caller_stack(move || async move {
                entered_clone.store(true, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                finished_clone.store(true, Ordering::SeqCst);
            })
            .await;
        });
        while !entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        caller.abort();
        let _ = caller.await;
        // Give the abort a scheduling opportunity, then confirm the inner
        // future never ran to completion.
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        assert!(!finished.load(Ordering::SeqCst));
    }
}
