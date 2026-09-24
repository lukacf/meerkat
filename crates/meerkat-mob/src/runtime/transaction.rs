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
                if Self::is_benign_compensation_absence(&error) {
                    tracing::debug!(
                        context = self.context,
                        compensation = %label,
                        %error,
                        "rollback compensation target was already absent"
                    );
                    continue;
                }
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

    fn is_benign_compensation_absence(error: &MobError) -> bool {
        matches!(
            error,
            MobError::CommsError(meerkat_core::comms::SendError::PeerNotFound(_))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fail_suppresses_already_absent_peer_compensation() {
        let mut rollback = LifecycleRollback::new("test rollback");
        rollback.defer("compensating peer notice", || async {
            Err(MobError::CommsError(
                meerkat_core::comms::SendError::PeerNotFound("peer-1".to_string()),
            ))
        });

        let error = rollback
            .fail(MobError::Internal("primary failure".to_string()))
            .await;

        assert!(
            matches!(error, MobError::Internal(message) if message == "primary failure"),
            "already-absent peer compensation should not obscure the primary failure"
        );
    }

    #[tokio::test]
    async fn fail_preserves_non_absence_compensation_failure() {
        let mut rollback = LifecycleRollback::new("test rollback");
        rollback.defer("compensating peer notice", || async {
            Err(MobError::CommsError(
                meerkat_core::comms::SendError::PeerOffline,
            ))
        });

        let error = rollback
            .fail(MobError::Internal("primary failure".to_string()))
            .await;

        assert!(
            matches!(error, MobError::Internal(message) if message.contains("rollback failures") && message.contains("peer offline")),
            "non-benign compensation failures should still be reported"
        );
    }
}
