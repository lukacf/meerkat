use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{MeerkatId, ProfileName};
use crate::runtime::MobState;
use meerkat_core::SessionError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Debug, Error)]
pub enum MobError {
    #[error("profile not found: {profile}")]
    ProfileNotFound { profile: ProfileName },

    #[error("meerkat not found: {meerkat_id}")]
    MeerkatNotFound { meerkat_id: MeerkatId },

    #[error("meerkat already exists: {meerkat_id}")]
    MeerkatAlreadyExists { meerkat_id: MeerkatId },

    #[error("meerkat not externally addressable: {meerkat_id}")]
    NotExternallyAddressable { meerkat_id: MeerkatId },

    #[error("invalid state transition from {from} to {to}")]
    InvalidTransition { from: MobState, to: MobState },

    #[error("wiring error between {a} and {b}: {reason}")]
    WiringError {
        a: MeerkatId,
        b: MeerkatId,
        reason: String,
    },

    #[error("definition invalid")]
    DefinitionError { diagnostics: Vec<Diagnostic> },

    #[error("storage error: {0}")]
    StorageError(#[from] Box<dyn std::error::Error + Send + Sync>),

    #[error("session error: {0}")]
    SessionError(#[from] SessionError),

    #[error("comms error: {0}")]
    CommsError(#[from] meerkat_core::comms::SendError),

    #[error("internal error: {0}")]
    Internal(String),
}

impl From<tokio::sync::mpsc::error::SendError<crate::runtime::MobCommand>> for MobError {
    fn from(_: tokio::sync::mpsc::error::SendError<crate::runtime::MobCommand>) -> Self {
        Self::Internal("mob command channel closed".to_string())
    }
}

impl From<tokio::sync::oneshot::error::RecvError> for MobError {
    fn from(_: tokio::sync::oneshot::error::RecvError) -> Self {
        Self::Internal("mob reply channel closed".to_string())
    }
}
