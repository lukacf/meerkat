//! §21 Run lifecycle events emitted by core.
//!
//! Core emits these during run execution. The runtime layer observes them
//! to transition InputState, emit RuntimeEvents, and manage the run lifecycle.

use serde::{Deserialize, Serialize};

use super::identifiers::{InputId, RunId};
use super::run_receipt::RunBoundaryReceipt;

/// Events emitted by core during run execution.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum RunEvent {
    /// A run has started.
    RunStarted {
        /// The ID of the run.
        run_id: RunId,
    },

    /// A boundary primitive was applied successfully.
    BoundaryApplied {
        /// The ID of the run.
        run_id: RunId,
        /// The receipt proving application.
        receipt: RunBoundaryReceipt,
    },

    /// A run completed successfully.
    RunCompleted {
        /// The ID of the run.
        run_id: RunId,
        /// Input IDs that were consumed during this run.
        consumed_input_ids: Vec<InputId>,
    },

    /// A run failed with an error.
    RunFailed {
        /// The ID of the run.
        run_id: RunId,
        /// Human-readable error description.
        error: String,
        /// Whether this failure is recoverable (transient).
        recoverable: bool,
    },

    /// A run was cancelled via `RunControlCommand::CancelCurrentRun`.
    RunCancelled {
        /// The ID of the run.
        run_id: RunId,
    },
}

impl RunEvent {
    /// Get the run ID for this event.
    pub fn run_id(&self) -> &RunId {
        match self {
            RunEvent::RunStarted { run_id }
            | RunEvent::BoundaryApplied { run_id, .. }
            | RunEvent::RunCompleted { run_id, .. }
            | RunEvent::RunFailed { run_id, .. }
            | RunEvent::RunCancelled { run_id } => run_id,
        }
    }

    /// Get contributing input IDs if this event carries them.
    pub fn contributing_input_ids(&self) -> &[InputId] {
        match self {
            RunEvent::BoundaryApplied { receipt, .. } => &receipt.contributing_input_ids,
            RunEvent::RunCompleted {
                consumed_input_ids, ..
            } => consumed_input_ids,
            _ => &[],
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::lifecycle::run_primitive::RunApplyBoundary;

    #[test]
    fn run_started_serde() {
        let event = RunEvent::RunStarted {
            run_id: RunId::new(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "run_started");
        let parsed: RunEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.run_id(), parsed.run_id());
    }

    #[test]
    fn boundary_applied_serde() {
        let receipt = RunBoundaryReceipt {
            run_id: RunId::new(),
            boundary: RunApplyBoundary::RunStart,
            contributing_input_ids: vec![InputId::new()],
            conversation_digest: Some("digest".into()),
            message_count: 3,
            sequence: 0,
        };
        let event = RunEvent::BoundaryApplied {
            run_id: receipt.run_id.clone(),
            receipt: receipt.clone(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "boundary_applied");
        let parsed: RunEvent = serde_json::from_value(json).unwrap();
        assert_eq!(
            parsed.contributing_input_ids(),
            receipt.contributing_input_ids.as_slice()
        );
    }

    #[test]
    fn run_completed_serde() {
        let ids = vec![InputId::new(), InputId::new()];
        let event = RunEvent::RunCompleted {
            run_id: RunId::new(),
            consumed_input_ids: ids.clone(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "run_completed");
        let parsed: RunEvent = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.contributing_input_ids(), ids.as_slice());
    }

    #[test]
    fn run_failed_serde() {
        let event = RunEvent::RunFailed {
            run_id: RunId::new(),
            error: "LLM timeout".into(),
            recoverable: true,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "run_failed");
        assert_eq!(json["recoverable"], true);
        let parsed: RunEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            RunEvent::RunFailed {
                recoverable: true,
                ..
            }
        ));
    }

    #[test]
    fn run_cancelled_serde() {
        let event = RunEvent::RunCancelled {
            run_id: RunId::new(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "run_cancelled");
        let parsed: RunEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, RunEvent::RunCancelled { .. }));
    }

    #[test]
    fn run_event_run_id_accessor() {
        let run_id = RunId::new();
        let events = vec![
            RunEvent::RunStarted {
                run_id: run_id.clone(),
            },
            RunEvent::RunCompleted {
                run_id: run_id.clone(),
                consumed_input_ids: vec![],
            },
            RunEvent::RunFailed {
                run_id: run_id.clone(),
                error: "err".into(),
                recoverable: false,
            },
            RunEvent::RunCancelled {
                run_id: run_id.clone(),
            },
        ];
        for event in &events {
            assert_eq!(event.run_id(), &run_id);
        }
    }

    #[test]
    fn contributing_input_ids_empty_for_non_carrying_events() {
        let event = RunEvent::RunStarted {
            run_id: RunId::new(),
        };
        assert!(event.contributing_input_ids().is_empty());

        let event = RunEvent::RunFailed {
            run_id: RunId::new(),
            error: "err".into(),
            recoverable: false,
        };
        assert!(event.contributing_input_ids().is_empty());

        let event = RunEvent::RunCancelled {
            run_id: RunId::new(),
        };
        assert!(event.contributing_input_ids().is_empty());
    }
}
