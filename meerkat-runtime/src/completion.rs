//! Input completion waiters — allows callers to await terminal outcome of an accepted input.
//!
//! When a surface accepts an input via the runtime, it can optionally receive a
//! `CompletionHandle` that resolves when the input reaches a terminal state
//! (Consumed or Abandoned). This bridges the async accept/await pattern needed
//! for surfaces that want synchronous-feeling turn execution through the runtime.
//!
//! `CompletionRegistry` is waiter plumbing only. Production code must never
//! treat waiter presence, waiter counts, or sender membership as semantic
//! runtime truth.

use std::collections::HashMap;
use std::future::Future;

use meerkat_core::TurnErrorMetadata;
use meerkat_core::lifecycle::InputId;
use meerkat_core::types::RunResult;
use serde_json::Value;

use crate::tokio::sync::oneshot;

/// Outcome delivered to a completion waiter.
#[derive(Debug)]
pub enum CompletionOutcome {
    /// The input was successfully consumed and produced a result.
    Completed(Box<RunResult>),
    /// The input was consumed but produced no RunResult (e.g. context-append ops).
    CompletedWithoutResult,
    /// The input reached a callback boundary and requires external tool
    /// fulfillment before the turn can continue.
    CallbackPending { tool_name: String, args: Value },
    /// The input reached the canonical cancellation terminal.
    Cancelled,
    /// The input was abandoned before completing.
    Abandoned(String),
    /// The input was abandoned before completing, with typed failure metadata.
    AbandonedWithError {
        reason: String,
        error: TurnErrorMetadata,
    },
    /// The turn produced a valid result, but a later runtime finalization step
    /// failed. Consumers can use the result while handling the mechanics error.
    CompletedWithFinalizationFailure {
        result: Box<RunResult>,
        error: TurnErrorMetadata,
    },
    /// The runtime was stopped or destroyed while the input was pending.
    RuntimeTerminated(String),
}

/// Snapshot of one input's registered completion waiters.
///
/// This is a diagnostic/supporting-carrier view only. Waiter counts are never
/// semantic runtime truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionWaiterEntrySnapshot {
    pub input_id: InputId,
    pub waiter_count: usize,
}

/// Diagnostic snapshot of the completion waiter registry.
///
/// This makes the carrier explicit for MeerkatMachine mapping work without
/// promoting waiter plumbing into canonical runtime semantics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionRegistrySnapshot {
    pub input_count: usize,
    pub waiter_count: usize,
    pub waiting_inputs: Vec<CompletionWaiterEntrySnapshot>,
}

/// Handle for awaiting the completion of an accepted input.
#[derive(Debug)]
pub struct CompletionHandle {
    rx: oneshot::Receiver<CompletionOutcome>,
}

impl CompletionHandle {
    /// Wait for the input to reach a terminal state.
    pub async fn wait(self) -> CompletionOutcome {
        match self.rx.await {
            Ok(outcome) => outcome,
            // Sender dropped without sending — runtime shut down unexpectedly
            Err(_) => CompletionOutcome::RuntimeTerminated(
                "completion channel closed without result".into(),
            ),
        }
    }

    /// Relay completion through a cleanup future before resolving the returned
    /// handle. This lets surfaces transfer cleanup ownership immediately after
    /// accepting runtime work while still returning a completion handle.
    pub fn with_cleanup<F, Fut>(self, cleanup: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        crate::tokio::spawn(async move {
            let outcome = self.wait().await;
            cleanup().await;
            let _ = tx.send(outcome);
        });
        Self { rx }
    }

    /// Relay completion through a cleanup future that can inspect the outcome.
    ///
    /// The cleanup future owns and returns the outcome so callers can run
    /// outcome-specific teardown without losing the completion result.
    pub fn with_outcome_cleanup<F, Fut>(self, cleanup: F) -> Self
    where
        F: FnOnce(CompletionOutcome) -> Fut + Send + 'static,
        Fut: Future<Output = CompletionOutcome> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        crate::tokio::spawn(async move {
            let outcome = self.wait().await;
            let outcome = cleanup(outcome).await;
            let _ = tx.send(outcome);
        });
        Self { rx }
    }

    /// Create a handle from a pre-resolved outcome.
    ///
    /// Used when the input is already terminal (e.g. dedup of completed input)
    /// and no waiter registration is needed.
    pub fn already_resolved(outcome: CompletionOutcome) -> Self {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(outcome);
        Self { rx }
    }
}

impl CompletionOutcome {
    pub fn abandoned_reason(&self) -> Option<&str> {
        match self {
            Self::Abandoned(reason) | Self::AbandonedWithError { reason, .. } => Some(reason),
            _ => None,
        }
    }

    pub fn error_metadata(&self) -> Option<&TurnErrorMetadata> {
        match self {
            Self::AbandonedWithError { error, .. }
            | Self::CompletedWithFinalizationFailure { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Registry of pending completion waiters, keyed by InputId.
///
/// Uses `Vec<Sender>` per InputId to support multiple waiters for the same input
/// (e.g. dedup of in-flight input registers a second waiter for the same InputId).
#[derive(Default)]
pub(crate) struct CompletionRegistry {
    waiters: HashMap<InputId, Vec<oneshot::Sender<CompletionOutcome>>>,
}

impl CompletionRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn take_waiters(
        &mut self,
        input_id: &InputId,
    ) -> Option<Vec<oneshot::Sender<CompletionOutcome>>> {
        self.waiters.remove(input_id)
    }

    /// Register a waiter for an input. Returns the handle the caller will await.
    ///
    /// Multiple waiters can be registered for the same InputId — all will be
    /// resolved when the input reaches a terminal state.
    pub(crate) fn register(&mut self, input_id: InputId) -> CompletionHandle {
        let (tx, rx) = oneshot::channel();
        self.waiters.entry(input_id).or_default().push(tx);
        CompletionHandle { rx }
    }

    /// Resolve all waiters for a completed input.
    pub(crate) fn resolve_completed(&mut self, input_id: &InputId, result: RunResult) {
        if let Some(senders) = self.take_waiters(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::Completed(Box::new(result.clone())));
            }
        }
    }

    /// Resolve all waiters for an input that completed without producing a RunResult.
    pub(crate) fn resolve_without_result(&mut self, input_id: &InputId) {
        if let Some(senders) = self.take_waiters(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::CompletedWithoutResult);
            }
        }
    }

    /// Resolve all waiters for an input that reached a callback boundary.
    pub(crate) fn resolve_callback_pending(
        &mut self,
        input_id: &InputId,
        tool_name: String,
        args: Value,
    ) {
        if let Some(senders) = self.take_waiters(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::CallbackPending {
                    tool_name: tool_name.clone(),
                    args: args.clone(),
                });
            }
        }
    }

    /// Resolve all waiters for an input that reached the cancellation terminal.
    pub(crate) fn resolve_cancelled(&mut self, input_id: &InputId) {
        if let Some(senders) = self.take_waiters(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::Cancelled);
            }
        }
    }

    /// Resolve all waiters for an abandoned input.
    pub(crate) fn resolve_abandoned(&mut self, input_id: &InputId, reason: String) {
        if let Some(senders) = self.take_waiters(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::Abandoned(reason.clone()));
            }
        }
    }

    /// Resolve all waiters for an abandoned input with typed failure metadata.
    pub(crate) fn resolve_abandoned_with_error(
        &mut self,
        input_id: &InputId,
        reason: String,
        error: TurnErrorMetadata,
    ) {
        if let Some(senders) = self.take_waiters(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::AbandonedWithError {
                    reason: reason.clone(),
                    error: error.clone(),
                });
            }
        }
    }

    /// Resolve all waiters for a turn whose output exists but finalization
    /// failed after output production.
    pub(crate) fn resolve_completed_with_finalization_failure(
        &mut self,
        input_id: &InputId,
        result: RunResult,
        error: TurnErrorMetadata,
    ) {
        if let Some(senders) = self.take_waiters(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::CompletedWithFinalizationFailure {
                    result: Box::new(result.clone()),
                    error: error.clone(),
                });
            }
        }
    }

    /// Resolve all pending waiters with a termination error.
    ///
    /// Used when the runtime is stopped or destroyed.
    pub(crate) fn resolve_all_terminated(&mut self, reason: &str) {
        for (_, senders) in self.waiters.drain() {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::RuntimeTerminated(reason.into()));
            }
        }
    }

    /// Resolve waiters whose input IDs are no longer pending after a
    /// lifecycle reconciliation (for example runtime recycle/recovery).
    pub(crate) fn resolve_not_pending<F>(&mut self, mut is_still_pending: F, reason: &str)
    where
        F: FnMut(&InputId) -> bool,
    {
        self.waiters.retain(|input_id, senders| {
            if is_still_pending(input_id) {
                return true;
            }

            for tx in senders.drain(..) {
                let _ = tx.send(CompletionOutcome::RuntimeTerminated(reason.into()));
            }
            false
        });
    }

    /// Snapshot the current waiter carrier without mutating it.
    pub(crate) fn diagnostic_snapshot(&self) -> CompletionRegistrySnapshot {
        let mut waiting_inputs: Vec<_> = self
            .waiters
            .iter()
            .map(|(input_id, senders)| CompletionWaiterEntrySnapshot {
                input_id: input_id.clone(),
                waiter_count: senders.len(),
            })
            .collect();
        waiting_inputs
            .sort_by(|left, right| left.input_id.to_string().cmp(&right.input_id.to_string()));

        CompletionRegistrySnapshot {
            input_count: waiting_inputs.len(),
            waiter_count: waiting_inputs.iter().map(|entry| entry.waiter_count).sum(),
            waiting_inputs,
        }
    }

    /// Check if there are any pending waiters.
    ///
    /// Test-only introspection. Production code must treat the registry as
    /// waiter plumbing rather than semantic runtime truth.
    #[cfg(test)]
    pub fn debug_has_waiters(&self) -> bool {
        !self.waiters.is_empty()
    }

    /// Number of pending waiters (total across all InputIds).
    ///
    /// Test-only introspection. Production code must treat the registry as
    /// waiter plumbing rather than semantic runtime truth.
    #[cfg(test)]
    pub fn debug_waiter_count(&self) -> usize {
        self.waiters.values().map(Vec::len).sum()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::types::{SessionId, Usage};

    fn make_run_result() -> RunResult {
        RunResult {
            text: "hello".into(),
            session_id: SessionId::new(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        }
    }

    #[tokio::test]
    async fn register_and_complete() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        assert!(registry.debug_has_waiters());
        assert_eq!(registry.debug_waiter_count(), 1);

        let result = make_run_result();
        registry.resolve_completed(&input_id, result);

        match handle.wait().await {
            CompletionOutcome::Completed(r) => assert_eq!(r.text, "hello"),
            other => panic!("Expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_and_abandon() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        registry.resolve_abandoned(&input_id, "retired".into());

        match handle.wait().await {
            CompletionOutcome::Abandoned(reason) => assert_eq!(reason, "retired"),
            other => panic!("Expected Abandoned, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_all_terminated() {
        let mut registry = CompletionRegistry::new();
        let h1 = registry.register(InputId::new());
        let h2 = registry.register(InputId::new());

        registry.resolve_all_terminated("runtime stopped");

        assert!(!registry.debug_has_waiters());

        match h1.wait().await {
            CompletionOutcome::RuntimeTerminated(r) => assert_eq!(r, "runtime stopped"),
            other => panic!("Expected RuntimeTerminated, got {other:?}"),
        }
        match h2.wait().await {
            CompletionOutcome::RuntimeTerminated(r) => assert_eq!(r, "runtime stopped"),
            other => panic!("Expected RuntimeTerminated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_nonexistent_is_a_noop() {
        let mut registry = CompletionRegistry::new();
        registry.resolve_completed(&InputId::new(), make_run_result());
        registry.resolve_abandoned(&InputId::new(), "gone".into());
        assert!(!registry.debug_has_waiters());
    }

    #[tokio::test]
    async fn dropped_sender_gives_terminated() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id);

        // Drop the registry (and thus the sender)
        drop(registry);

        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(_) => {}
            other => panic!("Expected RuntimeTerminated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multi_waiter_all_receive_result() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();

        let h1 = registry.register(input_id.clone());
        let h2 = registry.register(input_id.clone());
        let h3 = registry.register(input_id.clone());

        assert_eq!(registry.debug_waiter_count(), 3);

        let result = make_run_result();
        registry.resolve_completed(&input_id, result);

        assert!(!registry.debug_has_waiters());

        for handle in [h1, h2, h3] {
            match handle.wait().await {
                CompletionOutcome::Completed(r) => assert_eq!(r.text, "hello"),
                other => panic!("Expected Completed, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn resolve_without_result_sends_variant() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        registry.resolve_without_result(&input_id);

        match handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("Expected CompletedWithoutResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_without_result_multi_waiter() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let h1 = registry.register(input_id.clone());
        let h2 = registry.register(input_id.clone());

        registry.resolve_without_result(&input_id);

        for handle in [h1, h2] {
            match handle.wait().await {
                CompletionOutcome::CompletedWithoutResult => {}
                other => panic!("Expected CompletedWithoutResult, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn resolve_callback_pending_sends_variant() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        registry.resolve_callback_pending(
            &input_id,
            "browser".to_string(),
            serde_json::json!({ "url": "https://example.com" }),
        );

        match handle.wait().await {
            CompletionOutcome::CallbackPending { tool_name, args } => {
                assert_eq!(tool_name, "browser");
                assert_eq!(args, serde_json::json!({ "url": "https://example.com" }));
            }
            other => panic!("Expected CallbackPending, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_cancelled_sends_variant() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        registry.resolve_cancelled(&input_id);

        match handle.wait().await {
            CompletionOutcome::Cancelled => {}
            other => panic!("Expected Cancelled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn already_resolved_handle() {
        let handle = CompletionHandle::already_resolved(CompletionOutcome::CompletedWithoutResult);
        match handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("Expected CompletedWithoutResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn outcome_cleanup_observes_and_relays_result() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let observed = Arc::new(AtomicBool::new(false));
        let cleanup_observed = Arc::clone(&observed);
        let handle = CompletionHandle::already_resolved(CompletionOutcome::Abandoned(
            "apply failed: test".to_string(),
        ))
        .with_outcome_cleanup(move |outcome| async move {
            if matches!(&outcome, CompletionOutcome::Abandoned(reason) if reason == "apply failed: test")
            {
                cleanup_observed.store(true, Ordering::Release);
            }
            outcome
        });

        match handle.wait().await {
            CompletionOutcome::Abandoned(reason) => {
                assert_eq!(reason, "apply failed: test");
            }
            other => panic!("Expected Abandoned, got {other:?}"),
        }
        assert!(observed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn multi_waiter_terminated_on_reset() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let h1 = registry.register(input_id.clone());
        let h2 = registry.register(input_id);

        registry.resolve_all_terminated("runtime reset");

        for handle in [h1, h2] {
            match handle.wait().await {
                CompletionOutcome::RuntimeTerminated(r) => assert_eq!(r, "runtime reset"),
                other => panic!("Expected RuntimeTerminated, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn resolve_not_pending_keeps_pending_waiters() {
        let mut registry = CompletionRegistry::new();
        let keep_id = InputId::new();
        let drop_id = InputId::new();

        let keep_handle = registry.register(keep_id.clone());
        let drop_handle = registry.register(drop_id.clone());
        registry.resolve_not_pending(|input_id| input_id == &keep_id, "runtime recycled");
        assert_eq!(registry.debug_waiter_count(), 1);

        match drop_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(r) => assert_eq!(r, "runtime recycled"),
            other => panic!("Expected RuntimeTerminated, got {other:?}"),
        }

        registry.resolve_without_result(&keep_id);
        match keep_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("Expected CompletedWithoutResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_without_result_nonexistent_is_a_noop() {
        let mut registry = CompletionRegistry::new();
        registry.resolve_without_result(&InputId::new());
        assert!(!registry.debug_has_waiters());
    }
}
