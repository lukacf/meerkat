//! The exact attempt of one governed live add, observed against the router
//! that accepted it.

use std::sync::Weak;

use meerkat_core::mcp_attachment::{
    McpAttachmentAttemptObserver, McpAttachmentReadiness, McpStdioSetupFailure, McpStdioStderrTail,
};

use crate::McpRouterAdapter;
use crate::router::{AttemptReceiver, AttemptState, InstalledOperation};

/// Reads the attempt's own resolution, and for an installed attempt the
/// router's current connection and published routes. Holds the adapter
/// weakly: never a second owner of the router or its cleanup.
pub(crate) struct RouterAttemptObserver {
    adapter: Weak<McpRouterAdapter>,
    server: String,
    state: AttemptReceiver,
}

impl RouterAttemptObserver {
    pub(crate) fn new(
        adapter: Weak<McpRouterAdapter>,
        server: String,
        state: AttemptReceiver,
    ) -> Self {
        Self {
            adapter,
            server,
            state,
        }
    }

    async fn readiness(&self, tool: &str) -> McpAttachmentReadiness {
        let state = self.state.borrow().clone();
        match state {
            AttemptState::Pending => {
                // A router that dropped the attempt unresolved is gone.
                if self.state.has_changed().is_err() {
                    McpAttachmentReadiness::Retired
                } else {
                    McpAttachmentReadiness::Pending
                }
            }
            AttemptState::Rejected(detail) => McpAttachmentReadiness::Rejected { detail },
            AttemptState::Failed { detail, .. } => McpAttachmentReadiness::Failed { detail },
            AttemptState::Superseded => McpAttachmentReadiness::Superseded,
            AttemptState::Installed {
                connection_generation,
            } => {
                let Some(adapter) = self.adapter.upgrade() else {
                    return McpAttachmentReadiness::Retired;
                };
                match adapter
                    .installed_operation(&self.server, connection_generation, tool)
                    .await
                {
                    None => McpAttachmentReadiness::Retired,
                    Some(InstalledOperation::Callable) => McpAttachmentReadiness::Usable {
                        connection_generation,
                    },
                    Some(InstalledOperation::NotRouted) => McpAttachmentReadiness::ToolNotRouted {
                        connection_generation,
                    },
                    Some(InstalledOperation::Replaced) => McpAttachmentReadiness::Superseded,
                }
            }
        }
    }

    /// The typed stdio setup failure this attempt resolved with, if any.
    fn resolved_failure(&self) -> Option<McpStdioSetupFailure> {
        match &*self.state.borrow() {
            AttemptState::Failed {
                failure: Some(failure),
                ..
            } => Some(failure.clone()),
            _ => None,
        }
    }
}

#[async_trait::async_trait]
impl McpAttachmentAttemptObserver for RouterAttemptObserver {
    async fn observe(&self, tool: &str) -> McpAttachmentReadiness {
        self.readiness(tool).await
    }

    async fn wait(&self, tool: &str, timeout: std::time::Duration) -> McpAttachmentReadiness {
        let deadline = tokio::time::Instant::now() + timeout;
        // The deadline bounds every step, router lock acquisition included:
        // a router held elsewhere past it is a local not-settled answer.
        let bounded = |read| async move {
            tokio::time::timeout_at(deadline, read)
                .await
                .unwrap_or(McpAttachmentReadiness::Pending)
        };
        let mut state = self.state.clone();
        loop {
            // Drive the router's own result processing; the strong adapter
            // is held only for that step, never across the wait.
            let progress = match self.adapter.upgrade() {
                Some(adapter) => {
                    match tokio::time::timeout_at(deadline, adapter.progress_pending()).await {
                        Ok(progress) => progress,
                        Err(_) => return McpAttachmentReadiness::Pending,
                    }
                }
                None => None,
            };
            let readiness = bounded(self.readiness(tool)).await;
            if readiness != McpAttachmentReadiness::Pending
                || tokio::time::Instant::now() >= deadline
            {
                return readiness;
            }
            let Some(progress) = progress else {
                return bounded(self.readiness(tool)).await;
            };
            tokio::select! {
                () = progress.wait(Some(deadline)) => {}
                _ = state.changed() => {}
            }
        }
    }

    /// The class stays with the attempt; whether its tail is still retained
    /// is read from the router now (`false` once it is gone).
    async fn setup_failure(&self) -> Option<McpStdioSetupFailure> {
        let mut failure = self.resolved_failure()?;
        failure.stderr_tail_retained = match self.adapter.upgrade() {
            Some(adapter) => {
                adapter
                    .retains_stdio_tail(&self.server, failure.attempt)
                    .await
            }
            None => false,
        };
        Some(failure)
    }

    /// Only this exact attempt's tail, while the router retains it.
    async fn stderr_tail(&self) -> Option<McpStdioStderrTail> {
        let failure = self.resolved_failure()?;
        self.adapter
            .upgrade()?
            .stdio_stderr_tail_of(&self.server, failure.attempt)
            .await
    }
}
