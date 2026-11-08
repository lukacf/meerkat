//! Typed cleanup retained by the existing private service preparation owner.
use super::ServicePreparationOwner;
use crate::{
    ConnectError,
    resources::{LocalRoundOwner, PrivateLocalRoundAttempts},
};
use meerkat_core::{McpServerConfig, SessionId};
use meerkat_mcp::{
    McpAttemptGuardBounds, McpAttemptGuards, McpConstructionOwner, McpGuardCompletion, McpRouter,
    McpRouterAdapter, McpRouterShutdownObservation,
};
use std::{fmt, sync::Arc, time::Duration};

pub(super) struct PrivateCustody {
    session: SessionId,
    attempts: Arc<PrivateLocalRoundAttempts>,
    guards: McpAttemptGuards,
    close: tokio::sync::Mutex<Close>,
}
#[derive(Default)]
struct Close {
    observation: Option<McpRouterShutdownObservation>,
    joined: bool,
    failed: bool,
    diagnostic: Option<serde_json::Value>,
    diagnostic_batches: u64,
    terminal_diagnostics: Vec<serde_json::Value>,
    terminal_diagnostics_truncated: bool,
    unknown_completion_omitted: bool,
}
impl Close {
    fn retain_terminal(
        &mut self,
        completion: McpGuardCompletion,
        project: impl FnOnce() -> serde_json::Value,
    ) {
        match completion {
            McpGuardCompletion::Completed
            | McpGuardCompletion::Cancelled
            | McpGuardCompletion::Failed
            | McpGuardCompletion::Panicked => (),
            McpGuardCompletion::Held | McpGuardCompletion::TimedOut => return,
            _ => {
                self.unknown_completion_omitted = true;
                return;
            }
        }
        if self.terminal_diagnostics.len() < 8 {
            self.terminal_diagnostics.push(project());
        } else {
            self.terminal_diagnostics_truncated = true;
        }
    }
}
impl PrivateCustody {
    pub(super) fn new(router: McpRouter, session: SessionId) -> (McpRouter, Self) {
        let attempts = PrivateLocalRoundAttempts::new(session.clone(), router.construction_id());
        let guards = McpAttemptGuards::new(attempts.clone(), McpAttemptGuardBounds::default());
        let router = router.with_attempt_guards(
            guards.clone(),
            McpConstructionOwner::new(session.clone(), None),
        );
        (
            router,
            Self {
                session,
                attempts,
                guards,
                close: tokio::sync::Mutex::new(Close::default()),
            },
        )
    }

    pub(super) fn for_native_construction(session: SessionId) -> Arc<Self> {
        let attempts = PrivateLocalRoundAttempts::for_native_construction(session.clone());
        let guards = McpAttemptGuards::new(attempts.clone(), McpAttemptGuardBounds::default());
        Arc::new(Self {
            session,
            attempts,
            guards,
            close: tokio::sync::Mutex::new(Close::default()),
        })
    }

    pub(super) fn guards(&self) -> McpAttemptGuards {
        self.guards.clone()
    }

    pub(super) fn bind_construction(&self, adapter: &McpRouterAdapter) -> Result<(), ConnectError> {
        self.attempts.bind_construction(adapter.construction_id())
    }

    pub(super) fn matches_session(&self, session: &SessionId) -> bool {
        session == &self.session
    }

    pub(super) async fn diagnostic(&self) -> Option<serde_json::Value> {
        self.close.lock().await.diagnostic.clone()
    }

    pub(super) fn retain(
        &self,
        server: McpServerConfig,
        owner: LocalRoundOwner,
    ) -> Result<(), ConnectError> {
        self.attempts.retain(server, owner)
    }

    #[cfg(target_os = "linux")]
    pub(super) async fn stage_selected(
        &self,
        input: Arc<crate::protected_parent::selected_input::SelectedInput>,
        interrupted: impl std::future::Future<Output = std::io::Result<()>>,
    ) -> Result<(), ConnectError> {
        self.attempts.stage_selected(input, interrupted).await
    }

    #[cfg(target_os = "linux")]
    pub(super) fn staged_input(
        &self,
    ) -> Option<crate::protected_parent::selected_input::StagedSelectedInput> {
        self.attempts.staged_input()
    }

    pub(super) async fn close(&self, adapter: &McpRouterAdapter) -> Result<(), ()> {
        self.close_retained(Some(adapter)).await
    }

    /// No adapter was adopted, so no native connection was started. Join the
    /// same guarded preparation without fabricating a router shutdown report.
    pub(super) async fn close_unopened(&self) -> Result<(), ()> {
        self.close_retained(None).await
    }

    async fn close_retained(&self, adapter: Option<&McpRouterAdapter>) -> Result<(), ()> {
        self.attempts.fence();
        #[cfg(target_os = "linux")]
        let intake_failed = self.attempts.join_intake().await.is_err();
        // One retained owner serializes repeat observations of this same close.
        // Cancellation leaves the report and Core's tasks in that owner.
        let mut close = self.close.lock().await;
        #[cfg(target_os = "linux")]
        {
            close.failed |= intake_failed;
        }
        if close.observation.is_none()
            && let Some(adapter) = adapter
        {
            let observation = adapter.shutdown_with_report().await;
            match &observation {
                McpRouterShutdownObservation::Reported(report) => {
                    close.failed |= !report.is_settled();
                    for record in report.guards() {
                        match record.completion {
                            McpGuardCompletion::Completed
                            | McpGuardCompletion::Cancelled
                            | McpGuardCompletion::Held
                            | McpGuardCompletion::TimedOut => (),
                            _ => close.failed = true,
                        }
                    }
                }
                _ => close.failed = true,
            }
            close.observation = Some(observation);
        }
        while !close.joined {
            let records = self
                .guards
                .close_and_join(tokio::time::Instant::now() + Duration::from_secs(1))
                .await;
            close.diagnostic_batches = close.diagnostic_batches.saturating_add(1);
            // Core forgets terminal calls after each batch. Retain only bounded
            // scalar projections in this same owner before any following await.
            for record in records
                .iter()
                .filter(|row| row.key.server == "toolkit-setup")
            {
                close.retain_terminal(record.completion, || {
                    self.attempts.diagnostic_record(record)
                });
            }
            let mut pending = false;
            for record in &records {
                match record.completion {
                    McpGuardCompletion::Completed | McpGuardCompletion::Cancelled => (),
                    McpGuardCompletion::Held | McpGuardCompletion::TimedOut => pending = true,
                    McpGuardCompletion::Failed | McpGuardCompletion::Panicked => {
                        close.failed = true
                    }
                    _ => {
                        close.failed = true;
                        pending = true;
                    }
                }
            }
            close.joined = !pending;
            if pending {
                tokio::task::yield_now().await;
            }
        }
        close.failed |= self.attempts.settled().is_err();
        if close.diagnostic.is_none() {
            let mut diagnostic = self.attempts.diagnostic(&[]);
            diagnostic["core"] = serde_json::json!(close.terminal_diagnostics);
            diagnostic["core_truncated"] = serde_json::json!(close.terminal_diagnostics_truncated);
            diagnostic["core_scope"] = serde_json::json!("terminal_close_and_join_records");
            diagnostic["close_and_join_batches"] = serde_json::json!(close.diagnostic_batches);
            diagnostic["unknown_core_completion_omitted"] =
                serde_json::json!(close.unknown_completion_omitted);
            close.diagnostic = Some(diagnostic);
        }
        if close.failed { Err(()) } else { Ok(()) }
    }
}

// Failure retains the same report, guard installation and poisoned preparation.
// Dropping the error is never reported as successful shutdown or reuse.
pub(super) struct RetainedClose(pub Arc<ServicePreparationOwner>);
impl fmt::Debug for RetainedClose {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RetainedPrivateSetupClose")
            .finish_non_exhaustive()
    }
}
impl fmt::Display for RetainedClose {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _retained = &self.0;
        formatter.write_str("Private setup cleanup is unconfirmed; the same owner is retained")
    }
}
impl std::error::Error for RetainedClose {}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn terminal_diagnostics_survive_batches_without_transient_duplicates() {
        let mut close = Close::default();
        close.retain_terminal(McpGuardCompletion::Held, || {
            panic!("transient record is not projected")
        });
        close.retain_terminal(McpGuardCompletion::TimedOut, || {
            panic!("transient record is not projected")
        });
        close.retain_terminal(
            McpGuardCompletion::Completed,
            || serde_json::json!({"attempt":1,"stage":"begin"}),
        );
        // A later original close iteration reuses this owner; a Cancel row is
        // distinct from the same attempt's Begin and must not be deduplicated.
        close.retain_terminal(
            McpGuardCompletion::Cancelled,
            || serde_json::json!({"attempt":1,"stage":"cancel"}),
        );
        for attempt in 2..8 {
            close.retain_terminal(
                McpGuardCompletion::Failed,
                || serde_json::json!({"attempt":attempt,"stage":"cancel"}),
            );
        }
        assert_eq!(close.terminal_diagnostics.len(), 8);
        assert_eq!(close.terminal_diagnostics[0]["stage"], "begin");
        assert_eq!(close.terminal_diagnostics[1]["stage"], "cancel");
        assert!(!close.terminal_diagnostics_truncated);
        close.retain_terminal(McpGuardCompletion::Panicked, || {
            panic!("overflow is not projected")
        });
        assert!(close.terminal_diagnostics_truncated);
        assert_eq!(close.terminal_diagnostics.len(), 8);
        assert!(
            close.diagnostic.is_none(),
            "only actual joined close publishes the snapshot"
        );
        assert!(
            !close.failed && !close.joined,
            "diagnostic retention cannot change cleanup truth"
        );
    }
}

#[cfg(all(test, feature = "console", target_os = "linux"))]
mod selected_input_close_tests {
    use super::*;
    use crate::protected_parent::selected_input::inert_test_input;

    #[test]
    fn close_unopened_joins_dropped_selected_copy_and_retains_failure() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_files, owner, server) = LocalRoundOwner::selected_input_test_preparation();
            let custody = PrivateCustody::for_native_construction(SessionId::new());
            custody.retain(server, owner).unwrap();
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            entered_rx.await.unwrap();
            let input = inert_test_input("repository");
            let mut copy = Box::pin(custody.stage_selected(input.clone(), std::future::pending()));
            let copy_pending = futures_util::poll!(copy.as_mut()).is_pending();
            drop(copy);
            let mut close = Box::pin(custody.close_unopened());
            // Its retained copy is queued behind the blocker. Close must wait
            // on that exact task, even though its original waiter was dropped.
            let pending = futures_util::poll!(close.as_mut()).is_pending();
            release_tx.send(()).unwrap();
            blocker.await.unwrap();
            assert!(copy_pending);
            assert!(
                pending,
                "close cannot return before the retained copy joins"
            );
            assert!(close.await.is_err());
            assert!(input.failed_for_test());
            assert!(input.begin().is_err());
            assert!(custody.staged_input().is_none());
            assert!(custody.close.lock().await.joined);
            assert!(custody.attempts.settled().is_err());
            assert!(
                custody.close_unopened().await.is_err(),
                "same close retains its failure"
            );
        });
    }
}
