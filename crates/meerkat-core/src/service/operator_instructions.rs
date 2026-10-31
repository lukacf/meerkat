//! Operator-instruction extension of the session service.
//!
//! A session-owned target is written slot by slot by the session's own host
//! through this extension. A mob-owned one is a whole member snapshot its mob
//! delivers by generation; only the mob runtime records or releases it,
//! through its sealed member delivery, never through this extension or any
//! host or agent surface. Either is recorded in the session document only: a
//! live session's through its actor, a dormant session's store-only under its
//! hosting claim, which wakes and registers nothing. The next fresh run start
//! applies the pending changes. Services without a durable session document
//! refuse with [`SessionInstructionError::DurabilityUnavailable`].

use super::{SessionError, SessionService};
use crate::types::SessionId;
use crate::{
    EffectiveOperatorInstructions, InstructionSetErrorCode, InstructionStateError,
    InstructionWriteError, SessionInstructionReceipt, SessionInstructionState,
    SessionInstructionWrite,
};
use async_trait::async_trait;
use std::sync::Arc;

/// Why an operator-instruction operation on a session was refused.
#[derive(Debug, thiserror::Error)]
pub enum SessionInstructionError {
    /// The write or record itself was refused, typed.
    #[error(transparent)]
    Refused(#[from] InstructionWriteError),
    /// This service keeps no durable session document.
    #[error("operator instructions need a durable session; this service keeps none")]
    DurabilityUnavailable,
    /// The session could not be found, read or written.
    #[error(transparent)]
    Session(#[from] SessionError),
}

impl SessionInstructionError {
    /// The closed refusal code, when the refusal has one.
    #[must_use]
    pub fn code(&self) -> Option<InstructionSetErrorCode> {
        match self {
            Self::Refused(error) => Some(error.code()),
            Self::DurabilityUnavailable => Some(InstructionSetErrorCode::DurabilityUnavailable),
            Self::Session(_) => None,
        }
    }
}

impl From<InstructionStateError> for SessionInstructionError {
    fn from(error: InstructionStateError) -> Self {
        Self::Refused(InstructionWriteError::Malformed(error))
    }
}

/// A session's committed operator-instruction state: its recorded target
/// and the effective set its own entry rows leave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInstructionRead {
    pub target: Option<SessionInstructionState>,
    pub effective: EffectiveOperatorInstructions,
}

/// Operator-instruction extension for [`SessionService`]: a session's own
/// host writes its session-owned target and reads the committed state. A
/// mob member's snapshot is not written here: the mob runtime records it
/// through its sealed member delivery.
///
/// The write takes the service by `Arc`, as
/// [`SessionService::tool_application`] does, so an implementation can hand
/// a write to a task it owns: a caller that stops waiting must not leave a
/// write applied to a live session and missing from its session document.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionServiceInstructionExt: SessionService {
    /// Record one session-owned write in the session's target. The receipt
    /// returns once the target is in the session document; the write applies
    /// at the session's next fresh run start.
    async fn set_session_instruction(
        self: Arc<Self>,
        id: &SessionId,
        write: SessionInstructionWrite,
    ) -> Result<SessionInstructionReceipt, SessionInstructionError> {
        let _ = (id, write);
        Err(SessionInstructionError::DurabilityUnavailable)
    }

    /// Read the session's committed operator-instruction state without
    /// materializing it.
    async fn read_session_instructions(
        &self,
        id: &SessionId,
    ) -> Result<SessionInstructionRead, SessionInstructionError> {
        let _ = id;
        Err(SessionInstructionError::DurabilityUnavailable)
    }
}
