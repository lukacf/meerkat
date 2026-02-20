//! Error types for mob operations.

use crate::ids::{FlowId, MeerkatId, ProfileName};
use crate::runtime::MobState;
use crate::validate::Diagnostic;
use crate::{MobId, RunId, StepId};

/// Errors returned by mob operations.
#[derive(Debug, thiserror::Error)]
pub enum MobError {
    /// The requested profile does not exist in the mob definition.
    #[error("profile not found: {0}")]
    ProfileNotFound(ProfileName),

    /// The requested meerkat does not exist in the roster.
    #[error("meerkat not found: {0}")]
    MeerkatNotFound(MeerkatId),

    /// A meerkat with the given ID already exists.
    #[error("meerkat already exists: {0}")]
    MeerkatAlreadyExists(MeerkatId),

    /// The meerkat's profile does not allow external turns.
    #[error("meerkat is not externally addressable: {0}")]
    NotExternallyAddressable(MeerkatId),

    /// The requested lifecycle state transition is invalid.
    #[error("invalid state transition: {from} -> {to}")]
    InvalidTransition { from: MobState, to: MobState },

    /// A wiring operation failed.
    #[error("wiring error: {0}")]
    WiringError(String),

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

impl From<Box<dyn std::error::Error + Send + Sync>> for MobError {
    fn from(error: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::StorageError(error)
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
            MobError::MeerkatNotFound(MeerkatId::from("m")),
            MobError::MeerkatAlreadyExists(MeerkatId::from("m")),
            MobError::NotExternallyAddressable(MeerkatId::from("m")),
            MobError::InvalidTransition {
                from: MobState::Creating,
                to: MobState::Running,
            },
            MobError::WiringError("w".to_string()),
            MobError::DefinitionError(vec![]),
            MobError::FlowNotFound(FlowId::from("f")),
            MobError::FlowFailed {
                run_id: RunId::new(),
                reason: "r".to_string(),
            },
            MobError::RunNotFound(RunId::new()),
            MobError::RunCanceled(RunId::new()),
            MobError::FlowTurnTimedOut,
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
            MobError::ResetBarrier,
            MobError::StorageError(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "e",
            ))),
            MobError::SessionError(meerkat_core::service::SessionError::PersistenceDisabled),
            MobError::CommsError(meerkat_core::comms::SendError::PeerOffline),
            MobError::Internal("i".to_string()),
        ];
    }
}
