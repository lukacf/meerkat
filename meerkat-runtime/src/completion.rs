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
}

/// Registry of pending completion waiters, keyed by InputId.
#[derive(Default)]
pub struct CompletionRegistry {
    waiters: HashMap<InputId, oneshot::Sender<CompletionOutcome>>,
}

impl CompletionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a waiter for an input. Returns the handle the caller will await.
    pub fn register(&mut self, input_id: InputId) -> CompletionHandle {
        let (tx, rx) = oneshot::channel();
        self.waiters.insert(input_id, tx);
        CompletionHandle { rx }
    }

    /// Resolve a waiter for a completed input.
    ///
    /// Returns true if a waiter was found and resolved.
    pub fn resolve_completed(&mut self, input_id: &InputId, result: RunResult) -> bool {
        if let Some(tx) = self.waiters.remove(input_id) {
            let _ = tx.send(CompletionOutcome::Completed(result));
            true
        } else {
            false
        }
    }

    /// Resolve a waiter for an abandoned input.
    ///
    /// Returns true if a waiter was found and resolved.
    pub fn resolve_abandoned(&mut self, input_id: &InputId, reason: String) -> bool {
        if let Some(tx) = self.waiters.remove(input_id) {
            let _ = tx.send(CompletionOutcome::Abandoned(reason));
            true
        } else {
            false
        }
    }

    /// Resolve all pending waiters with a termination error.
    ///
    /// Used when the runtime is stopped or destroyed.
    pub fn resolve_all_terminated(&mut self, reason: &str) {
        for (_, tx) in self.waiters.drain() {
            let _ = tx.send(CompletionOutcome::RuntimeTerminated(reason.into()));
        }
    }

    /// Check if there are any pending waiters.
    pub fn has_pending(&self) -> bool {
        !self.waiters.is_empty()
    }

    /// Number of pending waiters.
    pub fn pending_count(&self) -> usize {
        self.waiters.len()
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
}
