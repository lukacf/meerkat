//! Surface-neutral safe-boundary instruction activation.
//!
//! This module does not own another activation lifecycle. It composes the
//! existing staged-session authority, generated runtime admission, stable
//! turn-finalization boundary, session actor mutation, and store commit.

use std::sync::Arc;

use meerkat_core::{
    InstructionActivationAdmissionErrorCode, InstructionActivationDisposition,
    InstructionActivationMutation, InstructionActivationReceipt, InstructionActivationRequest,
    SessionError, SessionId, SessionServiceHistoryExt as _,
};
use meerkat_runtime::{RuntimeDriverError, RuntimeStoreWriteFence};

use super::MeerkatSessionRuntime;

fn instruction_activation_session_error_to_host(
    error: SessionError,
) -> InstructionActivationHostError {
    match meerkat_runtime::instruction_activation_admission_for_session_error(&error) {
        Some((InstructionActivationAdmissionErrorCode::ExternalWriteFenceConflict, reason)) => {
            InstructionActivationHostError::ExternalWriteFenceConflict(reason)
        }
        Some((InstructionActivationAdmissionErrorCode::ExternalWriteFenceBackoff, reason)) => {
            InstructionActivationHostError::ExternalWriteFenceBackoff(reason)
        }
        Some((code, message)) => InstructionActivationHostError::Admission { code, message },
        None => InstructionActivationHostError::Session(error),
    }
}

impl From<meerkat_runtime::InstructionActivationRuntimeRefusal> for InstructionActivationHostError {
    fn from(refusal: meerkat_runtime::InstructionActivationRuntimeRefusal) -> Self {
        match refusal {
            meerkat_runtime::InstructionActivationRuntimeRefusal::Admission { code, message } => {
                Self::Admission { code, message }
            }
            meerkat_runtime::InstructionActivationRuntimeRefusal::Runtime(error) => {
                Self::Runtime(error)
            }
            other => Self::Runtime(RuntimeDriverError::Internal(other.to_string())),
        }
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
        if !self
            .service
            .has_live_session_under_runtime_turn_boundary(session_id)
            .await?
        {
            return Err(InstructionActivationHostError::Admission {
                code: InstructionActivationAdmissionErrorCode::TargetNotMaterialized,
                message: format!("session {session_id} is not currently materialized"),
            });
        }
        let identity = self.service.live_session_llm_identity(session_id).await?;
        meerkat_runtime::instruction_activation_runtime_admission(
            &self.runtime_adapter,
            session_id,
            Some(identity.model.as_str()),
        )
        .await?;

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
