//! §7 RuntimeEvent — full event hierarchy for runtime observability.
//!
//! Five categories: InputLifecycle, RunLifecycle, RuntimeState, Topology, Projection.

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::{InputId, RunId};
use serde::{Deserialize, Serialize};

use crate::identifiers::{
    CausationId, CorrelationId, EventCodeId, LogicalRuntimeId, RuntimeEventId,
};
use crate::input_state::InputLifecycleState;
use crate::runtime_state::RuntimeState;

/// Envelope for all runtime events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEventEnvelope {
    /// Unique event ID.
    pub id: RuntimeEventId,
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Which runtime emitted this event.
    pub runtime_id: LogicalRuntimeId,
    /// The event payload.
    pub event: RuntimeEvent,
    /// Causation chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<CausationId>,
    /// Correlation for cross-boundary tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
}

/// Runtime event categories and codes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "category", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeEvent {
    /// Input lifecycle state transitions.
    InputLifecycle(InputLifecycleEvent),
    /// Run lifecycle state transitions.
    RunLifecycle(RunLifecycleEvent),
    /// Runtime state transitions.
    RuntimeStateChange(RuntimeStateChangeEvent),
    /// Topology changes (agents joining/leaving).
    Topology(RuntimeTopologyEvent),
    /// Projection-derived events.
    Projection(RuntimeProjectionEvent),
}

impl RuntimeEvent {
    /// Get the stable event code for wire formats.
    pub fn event_code(&self) -> EventCodeId {
        match self {
            RuntimeEvent::InputLifecycle(e) => e.event_code(),
            RuntimeEvent::RunLifecycle(e) => e.event_code(),
            RuntimeEvent::RuntimeStateChange(_) => EventCodeId::new("runtime.state_changed"),
            RuntimeEvent::Topology(e) => e.event_code(),
            RuntimeEvent::Projection(_) => EventCodeId::new("runtime.projection_emitted"),
        }
    }
}

/// Input lifecycle events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputLifecycleEvent {
    /// Input accepted into the system.
    Accepted { input_id: InputId },
    /// Input deduplicated (idempotency key matched).
    Deduplicated {
        input_id: InputId,
        existing_id: InputId,
    },
    /// Input superseded by a newer input.
    Superseded {
        input_id: InputId,
        superseded_by: InputId,
    },
    /// Input coalesced into an aggregate.
    Coalesced {
        input_id: InputId,
        aggregate_id: InputId,
    },
    /// Input queued for processing.
    Queued { input_id: InputId },
    /// Input staged at a run boundary.
    Staged { input_id: InputId, run_id: RunId },
    /// Input applied (boundary primitive executed).
    Applied { input_id: InputId, run_id: RunId },
    /// Input consumed (run completed successfully).
    Consumed { input_id: InputId, run_id: RunId },
    /// Input abandoned (retire/reset/destroy).
    Abandoned { input_id: InputId, reason: String },
    /// Input state transitioned.
    StateTransitioned {
        input_id: InputId,
        from: InputLifecycleState,
        to: InputLifecycleState,
    },
}

impl InputLifecycleEvent {
    pub fn event_code(&self) -> EventCodeId {
        match self {
            Self::Accepted { .. } => EventCodeId::new("input.accepted"),
            Self::Deduplicated { .. } => EventCodeId::new("input.deduplicated"),
            Self::Superseded { .. } => EventCodeId::new("input.superseded"),
            Self::Coalesced { .. } => EventCodeId::new("input.coalesced"),
            Self::Queued { .. } => EventCodeId::new("input.queued"),
            Self::Staged { .. } => EventCodeId::new("input.staged"),
            Self::Applied { .. } => EventCodeId::new("input.applied"),
            Self::Consumed { .. } => EventCodeId::new("input.consumed"),
            Self::Abandoned { .. } => EventCodeId::new("input.abandoned"),
            Self::StateTransitioned { .. } => EventCodeId::new("input.state_transitioned"),
        }
    }
}

/// Run lifecycle events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RunLifecycleEvent {
    /// A run started.
    Started { run_id: RunId },
    /// A run completed successfully.
    Completed { run_id: RunId },
    /// A run failed.
    Failed {
        run_id: RunId,
        error: String,
        recoverable: bool,
    },
    /// A run was cancelled.
    Cancelled { run_id: RunId },
}

impl RunLifecycleEvent {
    pub fn event_code(&self) -> EventCodeId {
        match self {
            Self::Started { .. } => EventCodeId::new("run.started"),
            Self::Completed { .. } => EventCodeId::new("run.completed"),
            Self::Failed { .. } => EventCodeId::new("run.failed"),
            Self::Cancelled { .. } => EventCodeId::new("run.cancelled"),
        }
    }
}

/// Runtime state change events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStateChangeEvent {
    pub from: RuntimeState,
    pub to: RuntimeState,
}

/// Topology events (agents joining/leaving the runtime).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeTopologyEvent {
    /// A runtime instance was created.
    RuntimeCreated { runtime_id: LogicalRuntimeId },
    /// A runtime instance was retired.
    RuntimeRetired { runtime_id: LogicalRuntimeId },
    /// A runtime instance was respawned.
    RuntimeRespawned { runtime_id: LogicalRuntimeId },
    /// A runtime instance was destroyed.
    RuntimeDestroyed { runtime_id: LogicalRuntimeId },
}

impl RuntimeTopologyEvent {
    pub fn event_code(&self) -> EventCodeId {
        match self {
            Self::RuntimeCreated { .. } => EventCodeId::new("topology.runtime_created"),
            Self::RuntimeRetired { .. } => EventCodeId::new("topology.runtime_retired"),
            Self::RuntimeRespawned { .. } => EventCodeId::new("topology.runtime_respawned"),
            Self::RuntimeDestroyed { .. } => EventCodeId::new("topology.runtime_destroyed"),
        }
    }
}

/// Projection-derived events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeProjectionEvent {
    /// The projection rule that generated this event.
    pub rule_id: String,
    /// The projected input ID.
    pub projected_input_id: InputId,
    /// Source event that triggered the projection.
    pub source_event_id: RuntimeEventId,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn make_envelope(event: RuntimeEvent) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            id: RuntimeEventId::new(),
            timestamp: Utc::now(),
            runtime_id: LogicalRuntimeId::new("test-runtime"),
            event,
            causation_id: None,
            correlation_id: None,
        }
    }

    #[test]
    fn input_lifecycle_accepted_serde() {
        let event = RuntimeEvent::InputLifecycle(InputLifecycleEvent::Accepted {
            input_id: InputId::new(),
        });
        let envelope = make_envelope(event);
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["event"]["category"], "input_lifecycle");
        assert_eq!(json["event"]["data"]["code"], "accepted");
        let parsed: RuntimeEventEnvelope = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed.event,
            RuntimeEvent::InputLifecycle(InputLifecycleEvent::Accepted { .. })
        ));
    }

    #[test]
    fn input_lifecycle_deduplicated_serde() {
        let event = RuntimeEvent::InputLifecycle(InputLifecycleEvent::Deduplicated {
            input_id: InputId::new(),
            existing_id: InputId::new(),
        });
        let json = serde_json::to_value(&event).unwrap();
        let parsed: RuntimeEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            RuntimeEvent::InputLifecycle(InputLifecycleEvent::Deduplicated { .. })
        ));
    }

    #[test]
    fn input_lifecycle_superseded_serde() {
        let event = RuntimeEvent::InputLifecycle(InputLifecycleEvent::Superseded {
            input_id: InputId::new(),
            superseded_by: InputId::new(),
        });
        let json = serde_json::to_value(&event).unwrap();
        let parsed: RuntimeEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            RuntimeEvent::InputLifecycle(InputLifecycleEvent::Superseded { .. })
        ));
    }

    #[test]
    fn input_lifecycle_coalesced_serde() {
        let event = RuntimeEvent::InputLifecycle(InputLifecycleEvent::Coalesced {
            input_id: InputId::new(),
            aggregate_id: InputId::new(),
        });
        let json = serde_json::to_value(&event).unwrap();
        let parsed: RuntimeEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            RuntimeEvent::InputLifecycle(InputLifecycleEvent::Coalesced { .. })
        ));
    }

    #[test]
    fn run_lifecycle_started_serde() {
        let event = RuntimeEvent::RunLifecycle(RunLifecycleEvent::Started {
            run_id: RunId::new(),
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["category"], "run_lifecycle");
        let parsed: RuntimeEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            RuntimeEvent::RunLifecycle(RunLifecycleEvent::Started { .. })
        ));
    }

    #[test]
    fn run_lifecycle_failed_serde() {
        let event = RuntimeEvent::RunLifecycle(RunLifecycleEvent::Failed {
            run_id: RunId::new(),
            error: "timeout".into(),
            recoverable: true,
        });
        let json = serde_json::to_value(&event).unwrap();
        let parsed: RuntimeEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            RuntimeEvent::RunLifecycle(RunLifecycleEvent::Failed {
                recoverable: true,
                ..
            })
        ));
    }

    #[test]
    fn runtime_state_change_serde() {
        let event = RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from: RuntimeState::Idle,
            to: RuntimeState::Running,
        });
        let json = serde_json::to_value(&event).unwrap();
        let parsed: RuntimeEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
                from: RuntimeState::Idle,
                to: RuntimeState::Running,
            })
        ));
    }

    #[test]
    fn topology_created_serde() {
        let event = RuntimeEvent::Topology(RuntimeTopologyEvent::RuntimeCreated {
            runtime_id: LogicalRuntimeId::new("mob-agent-1"),
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["category"], "topology");
        let parsed: RuntimeEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            RuntimeEvent::Topology(RuntimeTopologyEvent::RuntimeCreated { .. })
        ));
    }

    #[test]
    fn event_code_coverage() {
        let events = vec![
            RuntimeEvent::InputLifecycle(InputLifecycleEvent::Accepted {
                input_id: InputId::new(),
            }),
            RuntimeEvent::RunLifecycle(RunLifecycleEvent::Completed {
                run_id: RunId::new(),
            }),
            RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
                from: RuntimeState::Idle,
                to: RuntimeState::Running,
            }),
            RuntimeEvent::Topology(RuntimeTopologyEvent::RuntimeRetired {
                runtime_id: LogicalRuntimeId::new("x"),
            }),
            RuntimeEvent::Projection(RuntimeProjectionEvent {
                rule_id: "rule-1".into(),
                projected_input_id: InputId::new(),
                source_event_id: RuntimeEventId::new(),
            }),
        ];
        for event in &events {
            let code = event.event_code();
            assert!(!code.0.is_empty());
        }
    }
}
