//! Input completion waiters — allows callers to await terminal outcome of an accepted input.
//!
//! When a surface accepts an input via the runtime, it can optionally receive a
//! `CompletionHandle` that resolves when the input reaches a terminal state
//! (Consumed or Abandoned). This bridges the async accept/await pattern needed
//! for surfaces that want synchronous-feeling turn execution through the runtime.

use std::collections::HashMap;

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::RunResult;

use crate::tokio::sync::oneshot;

/// Outcome delivered to a completion waiter.
#[derive(Debug)]
pub enum CompletionOutcome {
    /// The input was successfully consumed and produced a result.
    Completed(RunResult),
    /// The input was consumed but produced no RunResult (e.g. context-append ops).
    CompletedWithoutResult,
    /// The input was abandoned before completing.
    Abandoned(String),
    /// The runtime was stopped or destroyed while the input was pending.
    RuntimeTerminated(String),
}

/// Handle for awaiting the completion of an accepted input.
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

/// Registry of pending completion waiters, keyed by InputId.
///
/// Uses `Vec<Sender>` per InputId to support multiple waiters for the same input
/// (e.g. dedup of in-flight input registers a second waiter for the same InputId).
#[derive(Default)]
pub struct CompletionRegistry {
    waiters: HashMap<InputId, Vec<oneshot::Sender<CompletionOutcome>>>,
}

impl CompletionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a waiter for an input. Returns the handle the caller will await.
    ///
    /// Multiple waiters can be registered for the same InputId — all will be
    /// resolved when the input reaches a terminal state.
    pub fn register(&mut self, input_id: InputId) -> CompletionHandle {
        let (tx, rx) = oneshot::channel();
        self.waiters.entry(input_id).or_default().push(tx);
        CompletionHandle { rx }
    }

    /// Resolve all waiters for a completed input.
    ///
    /// Returns true if any waiters were found and resolved.
    pub fn resolve_completed(&mut self, input_id: &InputId, result: RunResult) -> bool {
        if let Some(senders) = self.waiters.remove(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::Completed(result.clone()));
            }
            true
        } else {
            false
        }
    }

    /// Resolve all waiters for an input that completed without producing a RunResult.
    ///
    /// Returns true if any waiters were found and resolved.
    pub fn resolve_without_result(&mut self, input_id: &InputId) -> bool {
        if let Some(senders) = self.waiters.remove(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::CompletedWithoutResult);
            }
            true
        } else {
            false
        }
    }

    /// Resolve all waiters for an abandoned input.
    ///
    /// Returns true if any waiters were found and resolved.
    pub fn resolve_abandoned(&mut self, input_id: &InputId, reason: String) -> bool {
        if let Some(senders) = self.waiters.remove(input_id) {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::Abandoned(reason.clone()));
            }
            true
        } else {
            false
        }
    }

    /// Resolve all pending waiters with a termination error.
    ///
    /// Used when the runtime is stopped or destroyed.
    pub fn resolve_all_terminated(&mut self, reason: &str) {
        for (_, senders) in self.waiters.drain() {
            for tx in senders {
                let _ = tx.send(CompletionOutcome::RuntimeTerminated(reason.into()));
            }
        }
    }

    /// Check if there are any pending waiters.
    pub fn has_pending(&self) -> bool {
        !self.waiters.is_empty()
    }

    /// Number of pending waiters (total across all InputIds).
    pub fn pending_count(&self) -> usize {
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
            structured_output: None,
            schema_warnings: None,
            skill_diagnostics: None,
        }
    }

    #[tokio::test]
    async fn register_and_complete() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        assert!(registry.has_pending());
        assert_eq!(registry.pending_count(), 1);

        let result = make_run_result();
        assert!(registry.resolve_completed(&input_id, result));

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

        assert!(registry.resolve_abandoned(&input_id, "retired".into()));

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

        assert!(!registry.has_pending());

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
    async fn resolve_nonexistent_returns_false() {
        let mut registry = CompletionRegistry::new();
        assert!(!registry.resolve_completed(&InputId::new(), make_run_result()));
        assert!(!registry.resolve_abandoned(&InputId::new(), "gone".into()));
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

        assert_eq!(registry.pending_count(), 3);

        let result = make_run_result();
        assert!(registry.resolve_completed(&input_id, result));

        assert!(!registry.has_pending());

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

        assert!(registry.resolve_without_result(&input_id));

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

        assert!(registry.resolve_without_result(&input_id));

        for handle in [h1, h2] {
            match handle.wait().await {
                CompletionOutcome::CompletedWithoutResult => {}
                other => panic!("Expected CompletedWithoutResult, got {other:?}"),
            }
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
    async fn resolve_without_result_nonexistent_returns_false() {
        let mut registry = CompletionRegistry::new();
        assert!(!registry.resolve_without_result(&InputId::new()));
    }
}
