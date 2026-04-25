//! Error types for mob operations.

use crate::ids::{AgentRuntimeId, FenceToken, FlowId, LoopId, MeerkatId, ProfileName, WorkRef};
use crate::runtime::MobState;
use crate::store::FrameAtomicOperation;
use crate::validate::Diagnostic;
use crate::{MobId, RunId, StepId};

/// Errors returned by mob operations.
#[derive(Debug, thiserror::Error)]
pub enum MobError {
    /// The requested profile does not exist in the mob definition.
    #[error("profile not found: {0}")]
    ProfileNotFound(ProfileName),

    /// The requested mob member does not exist in the roster.
    ///
    /// Renamed from `MeerkatNotFound` by DELETE_ME finding A2 + B8 as
    /// part of the 0.6 identity-first cascade. The inner type remains
    /// [`MeerkatId`] until the full A5 DSL-schema migration flips it to
    /// [`AgentIdentity`](crate::ids::AgentIdentity); the rename lands
    /// first so public error matching doesn't leak the legacy term.
    #[error("mob member not found: {0}")]
    MemberNotFound(MeerkatId),

    /// A mob member with the given ID already exists.
    ///
    /// Renamed from `MeerkatAlreadyExists` by DELETE_ME finding A2 + B8.
    #[error("mob member already exists: {0}")]
    MemberAlreadyExists(MeerkatId),

    /// The mob member's profile does not allow external turns.
    #[error("mob member is not externally addressable: {0}")]
    NotExternallyAddressable(MeerkatId),

    /// The requested lifecycle state transition is invalid.
    #[error("invalid state transition: {from} -> {to}")]
    InvalidTransition { from: MobState, to: MobState },

    /// A wiring operation failed.
    #[error("wiring error: {0}")]
    WiringError(String),

    /// The member failed to restore durable session state and is broken until repaired.
    #[error(
        "member {member_id} failed to restore {}: {reason}",
        format_member_restore_target(.session_id.as_ref())
    )]
    MemberRestoreFailed {
        member_id: MeerkatId,
        session_id: Option<meerkat_core::types::SessionId>,
        reason: String,
    },

    /// Waiting for kickoff completion timed out.
    #[error("kickoff wait timed out")]
    KickoffWaitTimedOut { pending_member_ids: Vec<MeerkatId> },

    /// Waiting for startup readiness timed out.
    #[error("member ready wait timed out")]
    ReadyWaitTimedOut { pending_member_ids: Vec<MeerkatId> },

    /// The mob definition failed validation.
    #[error("definition error: {}", format_diagnostics(.0))]
    DefinitionError(Vec<Diagnostic>),

    /// Referenced flow does not exist.
    #[error("flow not found: {0}")]
    FlowNotFound(FlowId),

    /// Run failed with a reason.
    #[error("flow failed for run {run_id}: {reason}")]
    FlowFailed { run_id: RunId, reason: String },

    /// Referenced run does not exist.
    #[error("run not found: {0}")]
    RunNotFound(RunId),

    /// Run was canceled.
    #[error("run canceled: {0}")]
    RunCanceled(RunId),

    /// Flow turn timed out while awaiting terminal transport outcome.
    #[error("flow turn timed out")]
    FlowTurnTimedOut,

    /// A frame-aware flow exceeded its configured nesting depth.
    #[error(
        "loop '{loop_id}' would exceed max_frame_depth={max_frame_depth} (current depth={current_depth})"
    )]
    FrameDepthLimitExceeded {
        loop_id: LoopId,
        max_frame_depth: u32,
        current_depth: u32,
    },

    /// The selected mob run store cannot provide frame-aware atomic persistence.
    #[error("mob run store cannot atomically persist frame operation '{operation}'")]
    FrameAtomicPersistenceUnavailable { operation: FrameAtomicOperation },

    /// Spec revision compare-and-swap failed.
    #[error("spec revision conflict for mob {mob_id}: expected {expected:?}, actual {actual}")]
    SpecRevisionConflict {
        mob_id: MobId,
        expected: Option<u64>,
        actual: u64,
    },

    /// Schema validation failed for a step output.
    #[error("schema validation failed for step {step_id}: {message}")]
    SchemaValidation { step_id: StepId, message: String },

    /// Not enough targets to satisfy dispatch/collection policy.
    #[error("insufficient targets for step {step_id}: required {required}, available {available}")]
    InsufficientTargets {
        step_id: StepId,
        required: u8,
        available: usize,
    },

    /// Topology policy denied a dispatch edge.
    #[error("topology violation: {from_role} -> {to_role}")]
    TopologyViolation {
        from_role: ProfileName,
        to_role: ProfileName,
    },

    /// Supervisor escalation happened.
    #[error("supervisor escalation: {0}")]
    SupervisorEscalation(String),

    /// Operation is not supported for the member's runtime mode.
    #[error("unsupported for runtime mode {mode}: {reason}")]
    UnsupportedForMode {
        mode: crate::MobRuntimeMode,
        reason: String,
    },

    /// Operation blocked by reset barrier.
    #[error("reset barrier active")]
    ResetBarrier,

    /// A storage operation failed.
    #[error("storage error: {0}")]
    StorageError(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A session service operation failed.
    #[error("session error: {0}")]
    SessionError(#[from] meerkat_core::service::SessionError),

    /// A comms operation failed.
    #[error("comms error: {0}")]
    CommsError(#[from] meerkat_core::comms::SendError),

    /// A runtime-backed member turn reached an external callback boundary.
    #[error("callback pending for session {session_id} on tool '{tool_name}'")]
    CallbackPending {
        session_id: meerkat_core::types::SessionId,
        tool_name: String,
        args: serde_json::Value,
    },

    /// The fence token does not match the member's current incarnation.
    #[error("stale fence token for {runtime_id}: expected {expected}, got {actual}")]
    StaleFenceToken {
        runtime_id: AgentRuntimeId,
        expected: FenceToken,
        actual: FenceToken,
    },

    /// A caller supplied an event replay cursor beyond the store frontier.
    #[error("stale mob event cursor: requested {after_cursor}, latest {latest_cursor}")]
    StaleEventCursor {
        after_cursor: u64,
        latest_cursor: u64,
    },

    /// The referenced work unit does not exist.
    #[error("work not found: {0}")]
    WorkNotFound(WorkRef),

    /// An internal error (unexpected state, logic errors).
    #[error("internal error: {0}")]
    Internal(String),
}

fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_member_restore_target(session_id: Option<&meerkat_core::types::SessionId>) -> String {
    match session_id {
        Some(session_id) => format!("session {session_id}"),
        None => "runtime bridge state".to_string(),
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for MobError {
    fn from(error: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::StorageError(error)
    }
}

impl From<crate::store::MobStoreError> for MobError {
    fn from(error: crate::store::MobStoreError) -> Self {
        match error {
            crate::store::MobStoreError::SpecRevisionConflict {
                mob_id,
                expected,
                actual,
            } => Self::SpecRevisionConflict {
                mob_id,
                expected,
                actual,
            },
            crate::store::MobStoreError::FrameAtomicPersistenceUnavailable { operation } => {
                Self::FrameAtomicPersistenceUnavailable { operation }
            }
            other => Self::StorageError(Box::new(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{Diagnostic, DiagnosticCode, DiagnosticSeverity};

    #[test]
    fn test_profile_not_found_display() {
        let err = MobError::ProfileNotFound(ProfileName::from("missing"));
        assert!(format!("{err}").contains("missing"));
    }

    /// DELETE_ME A2 + B8 regression: the `Meerkat*` variant prefix and
    /// the "meerkat" literal in error messages were renamed to
    /// identity-first terminology ("mob member"). This test pins both
    /// the display-string and the variant-construction shape so the
    /// 0.6 identity-first cascade cannot regress into legacy wording.
    #[test]
    fn member_not_found_and_already_exists_use_identity_first_display() {
        let not_found = MobError::MemberNotFound(MeerkatId::from("singer"));
        let already = MobError::MemberAlreadyExists(MeerkatId::from("singer"));
        let not_addressable = MobError::NotExternallyAddressable(MeerkatId::from("singer"));

        let msg_nf = format!("{not_found}");
        let msg_ae = format!("{already}");
        let msg_na = format!("{not_addressable}");

        assert_eq!(msg_nf, "mob member not found: singer");
        assert_eq!(msg_ae, "mob member already exists: singer");
        assert_eq!(msg_na, "mob member is not externally addressable: singer");

        // No legacy "meerkat" literal should appear in any of the
        // identity-first error displays.
        for msg in [&msg_nf, &msg_ae, &msg_na] {
            assert!(
                !msg.to_lowercase().contains("meerkat"),
                "identity-first mob errors must not carry legacy 'meerkat' wording: {msg}",
            );
        }
    }

    #[test]
    fn test_invalid_transition_display() {
        let err = MobError::InvalidTransition {
            from: MobState::Completed,
            to: MobState::Running,
        };
        let msg = format!("{err}");
        assert!(msg.contains("Completed"));
        assert!(msg.contains("Running"));
    }

    #[test]
    fn test_definition_error_display() {
        let err = MobError::DefinitionError(vec![
            Diagnostic {
                code: DiagnosticCode::MissingSkillRef,
                message: "skill 'foo' not found".to_string(),
                location: Some("profiles.worker.skills[0]".to_string()),
                severity: DiagnosticSeverity::Error,
            },
            Diagnostic {
                code: DiagnosticCode::MissingMcpRef,
                message: "mcp 'bar' not defined".to_string(),
                location: Some("profiles.worker.tools.mcp[0]".to_string()),
                severity: DiagnosticSeverity::Error,
            },
        ]);
        let msg = format!("{err}");
        assert!(msg.contains("missing_skill_ref"));
        assert!(msg.contains("missing_mcp_ref"));
    }

    #[test]
    fn test_session_error_from() {
        let session_err = meerkat_core::service::SessionError::NotFound {
            id: meerkat_core::types::SessionId::new(),
        };
        let mob_err: MobError = session_err.into();
        assert!(matches!(mob_err, MobError::SessionError(_)));
    }

    #[test]
    fn test_comms_error_from() {
        let send_err = meerkat_core::comms::SendError::PeerNotFound("agent-1".to_string());
        let mob_err: MobError = send_err.into();
        assert!(matches!(mob_err, MobError::CommsError(_)));
    }

    #[test]
    fn test_storage_error() {
        let err = MobError::StorageError(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "disk full",
        )));
        assert!(format!("{err}").contains("disk full"));
    }

    #[test]
    fn test_all_variants_exist() {
        // Ensures all variants are constructible.
        let _variants: Vec<MobError> = vec![
            MobError::ProfileNotFound(ProfileName::from("p")),
            MobError::MemberNotFound(MeerkatId::from("m")),
            MobError::MemberAlreadyExists(MeerkatId::from("m")),
            MobError::NotExternallyAddressable(MeerkatId::from("m")),
            MobError::InvalidTransition {
                from: MobState::Creating,
                to: MobState::Running,
            },
            MobError::WiringError("w".to_string()),
            MobError::MemberRestoreFailed {
                member_id: MeerkatId::from("m"),
                session_id: Some(meerkat_core::types::SessionId::new()),
                reason: "restore failed".to_string(),
            },
            MobError::KickoffWaitTimedOut {
                pending_member_ids: vec![MeerkatId::from("m")],
            },
            MobError::DefinitionError(vec![]),
            MobError::FlowNotFound(FlowId::from("f")),
            MobError::FlowFailed {
                run_id: RunId::new(),
                reason: "r".to_string(),
            },
            MobError::RunNotFound(RunId::new()),
            MobError::RunCanceled(RunId::new()),
            MobError::FlowTurnTimedOut,
            MobError::FrameDepthLimitExceeded {
                loop_id: LoopId::from("loop"),
                max_frame_depth: 1,
                current_depth: 1,
            },
            MobError::FrameAtomicPersistenceUnavailable {
                operation: FrameAtomicOperation::CasGrantNodeSlot,
            },
            MobError::SpecRevisionConflict {
                mob_id: MobId::from("mob"),
                expected: Some(2),
                actual: 3,
            },
            MobError::SchemaValidation {
                step_id: StepId::from("step"),
                message: "invalid".to_string(),
            },
            MobError::InsufficientTargets {
                step_id: StepId::from("step"),
                required: 2,
                available: 1,
            },
            MobError::TopologyViolation {
                from_role: ProfileName::from("lead"),
                to_role: ProfileName::from("worker"),
            },
            MobError::SupervisorEscalation("boom".to_string()),
            MobError::UnsupportedForMode {
                mode: crate::MobRuntimeMode::TurnDriven,
                reason: "autonomous host runtime required".to_string(),
            },
            MobError::ResetBarrier,
            MobError::StorageError(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "e",
            ))),
            MobError::SessionError(meerkat_core::service::SessionError::PersistenceDisabled),
            MobError::CommsError(meerkat_core::comms::SendError::PeerOffline),
            MobError::StaleFenceToken {
                runtime_id: crate::ids::AgentRuntimeId::initial(crate::ids::AgentIdentity::from(
                    "m",
                )),
                expected: FenceToken::new(1),
                actual: FenceToken::new(0),
            },
            MobError::WorkNotFound(WorkRef::new()),
            MobError::Internal("i".to_string()),
        ];
    }
}
