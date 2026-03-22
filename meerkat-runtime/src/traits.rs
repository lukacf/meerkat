//! §23 Runtime traits — RuntimeDriver and RuntimeControlPlane.
//!
//! These define the interface between surfaces and the runtime control-plane.

use meerkat_core::lifecycle::{InputId, RunEvent, RunId};
use serde::{Deserialize, Serialize};

use crate::accept::AcceptOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::InputState;
use crate::runtime_event::RuntimeEventEnvelope;
use crate::runtime_state::RuntimeState;

/// Errors from RuntimeDriver operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeDriverError {
    /// The runtime is not in a state that can accept this operation.
    #[error("Runtime not ready: {state}")]
    NotReady { state: RuntimeState },

    /// Input validation failed.
    #[error("Input validation failed: {reason}")]
    ValidationFailed { reason: String },

    /// The runtime has been destroyed.
    #[error("Runtime destroyed")]
    Destroyed,

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Errors from RuntimeControlPlane operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeControlPlaneError {
    /// Runtime not found.
    #[error("Runtime not found: {0}")]
    NotFound(LogicalRuntimeId),

    /// Invalid state for this operation.
    #[error("Invalid state for operation: {state}")]
    InvalidState { state: RuntimeState },

    /// Store error.
    #[error("Store error: {0}")]
    StoreError(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Runtime control commands (distinct from RunControlCommand which is core-level).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum RuntimeControlCommand {
    /// Stop the runtime gracefully.
    Stop,
    /// Resume the runtime after recovery.
    Resume,
}

/// Report from a recovery operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryReport {
    /// How many inputs were recovered.
    pub inputs_recovered: usize,
    /// How many inputs were abandoned during recovery.
    pub inputs_abandoned: usize,
    /// How many inputs were re-queued.
    pub inputs_requeued: usize,
    /// Details of recovery actions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
}

/// Report from a retire operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetireReport {
    /// How many non-terminal inputs were abandoned.
    pub inputs_abandoned: usize,
    /// How many inputs are pending drain (will be processed before stopping).
    #[serde(default)]
    pub inputs_pending_drain: usize,
}

/// Report from a reset operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetReport {
    /// How many non-terminal inputs were abandoned.
    pub inputs_abandoned: usize,
}

/// Report from a recycle operation (reset driver and recover state).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecycleReport {
    /// How many inputs were transferred to the new instance.
    pub inputs_transferred: usize,
}

/// Report from a destroy operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestroyReport {
    /// How many non-terminal inputs were abandoned.
    pub inputs_abandoned: usize,
}

/// The runtime driver — per-session interface for input acceptance and lifecycle.
///
/// Each session gets its own RuntimeDriver instance. The driver manages the
/// InputState ledger, policy resolution, and input queue for that session.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RuntimeDriver: Send + Sync {
    /// Validate that an input can be admitted without mutating driver state.
    ///
    /// Checks runtime phase and input durability constraints. Returns `Ok(())`
    /// if admission would proceed, or an error/rejection reason if not.
    /// This is used by atomic bundle admission to pre-validate all inputs
    /// before committing any of them.
    fn validate_pre_admission(&self, input: &Input) -> Result<(), RuntimeDriverError>;

    /// Accept an input into the runtime.
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError>;

    /// Handle a runtime event (from the event bus).
    async fn on_runtime_event(
        &mut self,
        event: RuntimeEventEnvelope,
    ) -> Result<(), RuntimeDriverError>;

    /// Handle a run event (from core).
    async fn on_run_event(&mut self, event: RunEvent) -> Result<(), RuntimeDriverError>;

    /// Handle a runtime control command.
    async fn on_runtime_control(
        &mut self,
        command: RuntimeControlCommand,
    ) -> Result<(), RuntimeDriverError>;

    /// Recover from a crash/restart.
    async fn recover(&mut self) -> Result<RecoveryReport, RuntimeDriverError>;

    /// Retire the runtime (no new input, abandon pending).
    async fn retire(&mut self) -> Result<RetireReport, RuntimeDriverError>;

    /// Reset the runtime (abandon all pending input, drain queue).
    async fn reset(&mut self) -> Result<ResetReport, RuntimeDriverError>;

    /// Destroy the runtime (terminal state, abandon all pending input).
    async fn destroy(&mut self) -> Result<DestroyReport, RuntimeDriverError>;

    /// Get the current runtime state.
    fn runtime_state(&self) -> RuntimeState;

    /// Get the state of a specific input.
    fn input_state(&self, input_id: &InputId) -> Option<&InputState>;

    /// List all non-terminal input IDs.
    fn active_input_ids(&self) -> Vec<InputId>;
}

/// The runtime control plane — manages multiple runtime instances.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RuntimeControlPlane: Send + Sync {
    /// Ingest an input into a specific runtime.
    async fn ingest(
        &self,
        runtime_id: &LogicalRuntimeId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeControlPlaneError>;

    /// Publish a runtime event.
    async fn publish_event(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(), RuntimeControlPlaneError>;

    /// Retire a runtime (no new input, drain existing).
    async fn retire(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RetireReport, RuntimeControlPlaneError>;

    /// Recycle a runtime (reset driver and recover state).
    async fn recycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecycleReport, RuntimeControlPlaneError>;

    /// Reset a runtime (abandon all pending input).
    async fn reset(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<ResetReport, RuntimeControlPlaneError>;

    /// Recover a runtime from crash.
    async fn recover(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecoveryReport, RuntimeControlPlaneError>;

    /// Get the state of a runtime.
    async fn runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RuntimeState, RuntimeControlPlaneError>;

    /// Destroy a runtime (terminal state, no recovery possible).
    async fn destroy(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<DestroyReport, RuntimeControlPlaneError>;

    /// Load a boundary receipt for verification.
    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<meerkat_core::lifecycle::RunBoundaryReceipt>, RuntimeControlPlaneError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // Verify traits are object-safe
    fn _assert_driver_object_safe(_: &dyn RuntimeDriver) {}
    fn _assert_control_plane_object_safe(_: &dyn RuntimeControlPlane) {}

    #[test]
    fn runtime_control_command_serde() {
        let cmd = RuntimeControlCommand::Stop;
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "stop");

        let cmd = RuntimeControlCommand::Resume;
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "resume");
    }

    #[test]
    fn runtime_driver_error_display() {
        let err = RuntimeDriverError::NotReady {
            state: RuntimeState::Initializing,
        };
        assert!(err.to_string().contains("initializing"));

        let err = RuntimeDriverError::ValidationFailed {
            reason: "bad input".into(),
        };
        assert!(err.to_string().contains("bad input"));
    }

    #[test]
    fn runtime_control_plane_error_display() {
        let err = RuntimeControlPlaneError::NotFound(LogicalRuntimeId::new("missing"));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn recovery_report_serde() {
        let report = RecoveryReport {
            inputs_recovered: 5,
            inputs_abandoned: 1,
            inputs_requeued: 3,
            details: vec!["requeued 3 staged inputs".into()],
        };
        let json = serde_json::to_value(&report).unwrap();
        let parsed: RecoveryReport = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.inputs_recovered, 5);
    }
}
