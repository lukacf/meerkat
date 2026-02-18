//! Error types for mob operations.

use crate::validate::Diagnostic;

/// Errors returned by mob operations.
#[derive(Debug, thiserror::Error)]
pub enum MobError {
    /// The requested profile does not exist in the mob definition.
    #[error("profile not found: {0}")]
    ProfileNotFound(String),

    /// The requested meerkat does not exist in the roster.
    #[error("meerkat not found: {0}")]
    MeerkatNotFound(String),

    /// A meerkat with the given ID already exists.
    #[error("meerkat already exists: {0}")]
    MeerkatAlreadyExists(String),

    /// The meerkat's profile does not allow external turns.
    #[error("meerkat is not externally addressable: {0}")]
    NotExternallyAddressable(String),

    /// The requested lifecycle state transition is invalid.
    #[error("invalid state transition: {from} -> {to}")]
    InvalidTransition {
        from: &'static str,
        to: &'static str,
    },

    /// A wiring operation failed.
    #[error("wiring error: {0}")]
    WiringError(String),

    /// The mob definition failed validation.
    #[error("definition error: {}", format_diagnostics(.0))]
    DefinitionError(Vec<Diagnostic>),

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{Diagnostic, DiagnosticCode};

    #[test]
    fn test_profile_not_found_display() {
        let err = MobError::ProfileNotFound("missing".to_string());
        assert!(format!("{err}").contains("missing"));
    }

    #[test]
    fn test_invalid_transition_display() {
        let err = MobError::InvalidTransition {
            from: "Completed",
            to: "Running",
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
            },
            Diagnostic {
                code: DiagnosticCode::MissingMcpRef,
                message: "mcp 'bar' not defined".to_string(),
                location: Some("profiles.worker.tools.mcp[0]".to_string()),
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
        // Ensures all 11 variants are constructible
        let _variants: Vec<MobError> = vec![
            MobError::ProfileNotFound("p".to_string()),
            MobError::MeerkatNotFound("m".to_string()),
            MobError::MeerkatAlreadyExists("m".to_string()),
            MobError::NotExternallyAddressable("m".to_string()),
            MobError::InvalidTransition { from: "A", to: "B" },
            MobError::WiringError("w".to_string()),
            MobError::DefinitionError(vec![]),
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
