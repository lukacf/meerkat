//! Surface-neutral safe-boundary instruction activation.
//!
//! This module does not own another activation lifecycle. It composes the
//! existing staged-session authority, generated runtime admission, stable
//! turn-finalization boundary, session actor mutation, and store commit.

use std::sync::Arc;

use meerkat_core::{
    InstructionActivationAdmissionErrorCode, InstructionActivationDisposition,
    InstructionActivationMutation, InstructionActivationReceipt, InstructionActivationRequest,
    SessionError, SessionId, SessionService as _, SessionServiceHistoryExt as _,
};
use meerkat_runtime::{
    RuntimeDriverError, RuntimeState, RuntimeStoreWriteFence, SessionServiceRuntimeExt as _,
};

use super::MeerkatSessionRuntime;

fn instruction_activation_session_error_to_host(
    error: SessionError,
) -> InstructionActivationHostError {
    match error {
        SessionError::ExternalWriteFenceConflict { reason } => {
            InstructionActivationHostError::ExternalWriteFenceConflict(reason)
        }
        SessionError::ExternalWriteFenceBackoff { reason } => {
            InstructionActivationHostError::ExternalWriteFenceBackoff(reason)
        }
        SessionError::Unsupported(message) => InstructionActivationHostError::Admission {
            code: InstructionActivationAdmissionErrorCode::DurabilityUnavailable,
            message,
        },
        other => InstructionActivationHostError::Session(other),
    }
}

/// Surface-neutral failure from the activation composition host.
#[derive(Debug, thiserror::Error)]
pub enum InstructionActivationHostError {
    #[error("instruction activation admission rejected ({code:?}): {message}")]
    Admission {
        code: InstructionActivationAdmissionErrorCode,
        message: String,
    },
    #[error("instruction activation external write fence conflicted: {0}")]
    ExternalWriteFenceConflict(String),
    #[error("instruction activation external write fence requested backoff: {0}")]
    ExternalWriteFenceBackoff(String),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("instruction activation runtime authority failed: {0}")]
    Runtime(#[source] RuntimeDriverError),
    #[error("instruction activation owner task failed: {0}")]
    OwnerTask(String),
}

impl InstructionActivationHostError {
    #[must_use]
    pub const fn admission_code(&self) -> Option<InstructionActivationAdmissionErrorCode> {
        match self {
            Self::Admission { code, .. } => Some(*code),
            Self::ExternalWriteFenceConflict(_) => {
                Some(InstructionActivationAdmissionErrorCode::ExternalWriteFenceConflict)
            }
            Self::ExternalWriteFenceBackoff(_) => {
                Some(InstructionActivationAdmissionErrorCode::ExternalWriteFenceBackoff)
            }
            Self::Session(_) | Self::Runtime(_) | Self::OwnerTask(_) => None,
        }
    }
}

impl MeerkatSessionRuntime {
    /// Activate an immutable instruction revision through the canonical
    /// materialized-session boundary.
    pub async fn activate_instruction(
        &self,
        session_id: &SessionId,
        request: InstructionActivationRequest,
    ) -> Result<InstructionActivationReceipt, InstructionActivationHostError> {
        self.activate_instruction_inner(session_id, request, None)
            .await
    }

    /// Read canonical durable activation records from the persistent session
    /// owner. This is a transcript read, not a materialization-status cache.
    pub async fn read_instruction_activations(
        &self,
        session_id: &SessionId,
        query: meerkat_core::InstructionActivationReadQuery,
    ) -> Result<meerkat_core::InstructionActivationReadPage, SessionError> {
        self.service
            .read_instruction_activation_records(session_id, query)
            .await
    }

    /// Activate while an external authority fence is retained by the selected
    /// RuntimeStore across the physical prepared-session publication.
    pub async fn activate_instruction_with_write_fence(
        &self,
        session_id: &SessionId,
        request: InstructionActivationRequest,
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<InstructionActivationReceipt, InstructionActivationHostError> {
        self.activate_instruction_inner(session_id, request, Some(write_fence))
            .await
    }

    async fn activate_instruction_inner(
        &self,
        session_id: &SessionId,
        request: InstructionActivationRequest,
        write_fence: Option<Arc<dyn RuntimeStoreWriteFence>>,
    ) -> Result<InstructionActivationReceipt, InstructionActivationHostError> {
        if self.staged_sessions.contains(session_id).await {
            return Err(InstructionActivationHostError::Admission {
                code: InstructionActivationAdmissionErrorCode::TargetNotMaterialized,
                message: format!("session {session_id} is staged and not materialized"),
            });
        }

        #[cfg(feature = "live")]
        let live_lifecycle_lease = self
            .runtime_adapter
            .acquire_live_open_lifecycle_lease(session_id)
            .await
            .map_err(InstructionActivationHostError::Runtime)?;
        let turn_boundary = self
            .service
            .acquire_runtime_turn_finalization_guard(session_id)
            .await;
        if !self.service.has_live_session(session_id).await? {
            return Err(InstructionActivationHostError::Admission {
                code: InstructionActivationAdmissionErrorCode::TargetNotMaterialized,
                message: format!("session {session_id} is not currently materialized"),
            });
        }
        #[cfg(feature = "live")]
        if self
            .runtime_adapter
            .live_active_channel_for_session(session_id)
            .await
            .is_some()
        {
            return Err(InstructionActivationHostError::Admission {
                code: InstructionActivationAdmissionErrorCode::LiveChannelOpen,
                message: format!("session {session_id} has an open live channel"),
            });
        }

        let runtime_running = self
            .runtime_adapter
            .runtime_state(session_id)
            .await
            .is_ok_and(|state| matches!(state, RuntimeState::Running));
        let has_active_inputs = self
            .runtime_adapter
            .list_active_inputs(session_id)
            .await
            .is_ok_and(|inputs| !inputs.is_empty());
        if !matches!(
            self.runtime_adapter
                .resolve_transcript_edit_admission(session_id, runtime_running, has_active_inputs)
                .await,
            Ok(meerkat_runtime::meerkat_machine::dsl::TranscriptEditAdmissionKind::Admissible)
        ) {
            return Err(InstructionActivationHostError::Admission {
                code: InstructionActivationAdmissionErrorCode::SessionBusy,
                message: format!("session {session_id} has active runtime work"),
            });
        }

        let identity = self.service.live_session_llm_identity(session_id).await?;
        let resolved_capabilities = self
            .runtime_adapter
            .resolved_session_llm_capabilities(session_id)
            .await
            .map_err(InstructionActivationHostError::Runtime)?
            .ok_or_else(|| {
                InstructionActivationHostError::Runtime(RuntimeDriverError::Internal(format!(
                    "materialized session {session_id} has no machine-owned resolved llm capability surface"
                )))
            })?;
        if !resolved_capabilities.supports_mid_conversation_system_messages {
            return Err(InstructionActivationHostError::Admission {
                code: InstructionActivationAdmissionErrorCode::UnsupportedCurrentLowering,
                message: format!(
                    "model {} for session {session_id} cannot exactly represent an ordered mid-conversation System activation",
                    identity.model
                ),
            });
        }

        let service = Arc::clone(&self.service);
        let owned_session_id = session_id.clone();
        let mutation = tokio::spawn(async move {
            let result = service
                .activate_instruction_under_runtime_turn_boundary(
                    &owned_session_id,
                    request,
                    write_fence,
                )
                .await
                .map_err(instruction_activation_session_error_to_host);
            drop(turn_boundary);
            #[cfg(feature = "live")]
            drop(live_lifecycle_lease);
            result
        })
        .await
        .map_err(|error| InstructionActivationHostError::OwnerTask(error.to_string()))??;
        let (record, disposition) = match mutation {
            InstructionActivationMutation::Appended(record) => {
                (record, InstructionActivationDisposition::Applied)
            }
            InstructionActivationMutation::Duplicate(record) => {
                (record, InstructionActivationDisposition::Duplicate)
            }
        };
        Ok(InstructionActivationReceipt {
            record,
            disposition,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_fenced_store_is_typed_durability_admission() {
        let error = instruction_activation_session_error_to_host(SessionError::Unsupported(
            "fenced session boundary is unsupported".to_string(),
        ));
        assert_eq!(
            error.admission_code(),
            Some(InstructionActivationAdmissionErrorCode::DurabilityUnavailable)
        );
    }
}
