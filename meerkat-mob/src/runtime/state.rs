#[cfg(test)]
use super::orchestrator_kernel::MobOrchestratorSnapshot;
use super::*;
use crate::run::MobRun;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// MobState
// ---------------------------------------------------------------------------

/// Lifecycle state of a mob, stored as `Arc<AtomicU8>` for lock-free reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MobState {
    Creating = 0,
    Running = 1,
    Stopped = 2,
    Completed = 3,
    Destroyed = 4,
}

impl MobState {
    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Creating,
            1 => Self::Running,
            2 => Self::Stopped,
            3 => Self::Completed,
            4 => Self::Destroyed,
            _ => {
                debug_assert!(false, "invalid mob lifecycle state byte: {v}");
                tracing::error!(state_byte = v, "invalid mob lifecycle state byte");
                Self::Destroyed
            }
        }
    }

    /// Human-readable name for the state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "Creating",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Completed => "Completed",
            Self::Destroyed => "Destroyed",
        }
    }
}

impl std::fmt::Display for MobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Narrow lifecycle owner for top-level mob state and tracked flow cleanup.
#[derive(Clone)]
pub(super) struct MobLifecycleOwner {
    state: Arc<AtomicU8>,
    tracked_flows: Arc<AtomicUsize>,
}

impl MobLifecycleOwner {
    pub(super) fn new(state: Arc<AtomicU8>) -> Self {
        Self {
            state,
            tracked_flows: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(super) fn state(&self) -> MobState {
        MobState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub(super) fn expect_transition(
        &self,
        expected: &[MobState],
        to: MobState,
    ) -> Result<(), MobError> {
        let current = self.state();
        if !expected.contains(&current) {
            return Err(MobError::InvalidTransition { from: current, to });
        }
        Ok(())
    }

    pub(super) fn require_state(&self, allowed: &[MobState]) -> Result<(), MobError> {
        let current = self.state();
        if !allowed.contains(&current) {
            return Err(MobError::InvalidTransition {
                from: current,
                to: allowed[0],
            });
        }
        Ok(())
    }

    pub(super) fn set_state(&self, next: MobState) {
        self.state.store(next as u8, Ordering::Release);
    }

    pub(super) fn register_tracked_flow(&self) {
        self.tracked_flows.fetch_add(1, Ordering::AcqRel);
    }

    pub(super) fn finish_tracked_flow(&self) {
        let _ = self
            .tracked_flows
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_sub(1))
            });
    }

    pub(super) fn clear_tracked_flows(&self) {
        self.tracked_flows.store(0, Ordering::Release);
    }

    pub(super) fn tracked_flow_count(&self) -> usize {
        self.tracked_flows.load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------------------
// MobCommand
// ---------------------------------------------------------------------------

/// Commands sent from [`MobHandle`] to the [`MobActor`] for serialized processing.
pub(super) enum MobCommand {
    Spawn {
        spec: super::handle::SpawnMemberSpec,
        reply_tx: oneshot::Sender<Result<MemberRef, MobError>>,
    },
    SpawnProvisioned {
        spawn_ticket: u64,
        result: Result<MemberRef, MobError>,
    },
    Retire {
        meerkat_id: MeerkatId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Respawn {
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    RetireAll {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Wire {
        a: MeerkatId,
        b: MeerkatId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Unwire {
        a: MeerkatId,
        b: MeerkatId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ExternalTurn {
        meerkat_id: MeerkatId,
        message: meerkat_core::types::ContentInput,
        handling_mode: meerkat_core::types::HandlingMode,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        reply_tx: oneshot::Sender<Result<SessionId, MobError>>,
    },
    InternalTurn {
        meerkat_id: MeerkatId,
        message: meerkat_core::types::ContentInput,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    RunFlow {
        flow_id: FlowId,
        activation_params: serde_json::Value,
        scoped_event_tx: Option<tokio::sync::mpsc::Sender<meerkat_core::ScopedAgentEvent>>,
        reply_tx: oneshot::Sender<Result<RunId, MobError>>,
    },
    CancelFlow {
        run_id: RunId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    FlowStatus {
        run_id: RunId,
        reply_tx: oneshot::Sender<Result<Option<MobRun>, MobError>>,
    },
    FlowFinished {
        run_id: RunId,
    },
    #[cfg(test)]
    FlowTrackerCounts {
        reply_tx: oneshot::Sender<(usize, usize)>,
    },
    #[cfg(test)]
    OrchestratorSnapshot {
        reply_tx: oneshot::Sender<MobOrchestratorSnapshot>,
    },
    Stop {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ResumeLifecycle {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Complete {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Destroy {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Reset {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    TaskCreate {
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
        reply_tx: oneshot::Sender<Result<TaskId, MobError>>,
    },
    TaskUpdate {
        task_id: TaskId,
        status: TaskStatus,
        owner: Option<MeerkatId>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    SetSpawnPolicy {
        policy: Option<Arc<dyn super::spawn_policy::SpawnPolicy>>,
        reply_tx: oneshot::Sender<()>,
    },
    Shutdown {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_owner_rejects_invalid_transition() {
        let owner = MobLifecycleOwner::new(Arc::new(AtomicU8::new(MobState::Running as u8)));
        let err = owner
            .expect_transition(&[MobState::Stopped], MobState::Running)
            .expect_err("running should not satisfy stopped-only transition");
        assert!(matches!(
            err,
            MobError::InvalidTransition {
                from: MobState::Running,
                to: MobState::Running
            }
        ));
    }

    #[test]
    fn lifecycle_owner_tracks_flows_without_underflow() {
        let owner = MobLifecycleOwner::new(Arc::new(AtomicU8::new(MobState::Running as u8)));
        owner.register_tracked_flow();
        owner.register_tracked_flow();
        assert_eq!(owner.tracked_flow_count(), 2);

        owner.finish_tracked_flow();
        owner.finish_tracked_flow();
        owner.finish_tracked_flow();
        assert_eq!(owner.tracked_flow_count(), 0);
    }
}
