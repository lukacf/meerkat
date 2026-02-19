use super::*;

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

// ---------------------------------------------------------------------------
// MobCommand
// ---------------------------------------------------------------------------

/// Commands sent from [`MobHandle`] to the [`MobActor`] for serialized processing.
pub(super) enum MobCommand {
    Spawn {
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
        backend: Option<MobBackendKind>,
        reply_tx: oneshot::Sender<Result<MemberRef, MobError>>,
    },
    Retire {
        meerkat_id: MeerkatId,
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
        message: String,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    InternalTurn {
        meerkat_id: MeerkatId,
        message: String,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
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
    TaskCreate {
        subject: String,
        description: String,
        blocked_by: Vec<String>,
        reply_tx: oneshot::Sender<Result<String, MobError>>,
    },
    TaskUpdate {
        task_id: String,
        status: TaskStatus,
        owner: Option<MeerkatId>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Shutdown {
        reply_tx: oneshot::Sender<()>,
    },
}
