use crate::error::MobError;
use std::future::Future;
use std::pin::Pin;

#[cfg(not(target_arch = "wasm32"))]
type RollbackFuture<'a> = Pin<Box<dyn Future<Output = Result<(), MobError>> + Send + 'a>>;
#[cfg(not(target_arch = "wasm32"))]
type RollbackAction<'a> = Box<dyn FnOnce() -> RollbackFuture<'a> + Send + 'a>;

#[cfg(target_arch = "wasm32")]
type RollbackFuture<'a> = Pin<Box<dyn Future<Output = Result<(), MobError>> + 'a>>;
#[cfg(target_arch = "wasm32")]
type RollbackAction<'a> = Box<dyn FnOnce() -> RollbackFuture<'a> + 'a>;

/// Shared transactional rollback guard for lifecycle operations.
///
/// Registered compensations execute in reverse order to preserve stack-like
/// unwinding semantics when an operation fails mid-flight.
pub(super) struct LifecycleRollback<'a> {
    context: &'static str,
    actions: Vec<(String, RollbackAction<'a>)>,
}

impl<'a> LifecycleRollback<'a> {
    pub(super) fn new(context: &'static str) -> Self {
        Self {
            context,
            actions: Vec::new(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn defer<F, Fut>(&mut self, label: impl Into<String>, action: F)
    where
        F: FnOnce() -> Fut + Send + 'a,
        Fut: Future<Output = Result<(), MobError>> + Send + 'a,
    {
        self.actions
            .push((label.into(), Box::new(move || Box::pin(action()))));
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn defer<F, Fut>(&mut self, label: impl Into<String>, action: F)
    where
        F: FnOnce() -> Fut + 'a,
        Fut: Future<Output = Result<(), MobError>> + 'a,
    {
        self.actions
            .push((label.into(), Box::new(move || Box::pin(action()))));
    }

    pub(super) async fn fail(mut self, primary: MobError) -> MobError {
        let mut rollback_errors = Vec::new();
        while let Some((label, action)) = self.actions.pop() {
            if let Err(error) = action().await {
                rollback_errors.push(format!("{label}: {error}"));
            }
        }

        if rollback_errors.is_empty() {
            return primary;
        }

        MobError::Internal(format!(
            "{} failed: {primary}; rollback failures: {}",
            self.context,
            rollback_errors.join("; ")
        ))
    }
}
