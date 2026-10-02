//! Member-level safe-boundary instruction activation.
//!
//! A host that changes a member's standing instructions after the member's
//! session was minted (a restored member, a mid-life spec change) cannot use
//! build-time instructions: resume inherits persisted prompt state. This is
//! the member-level door onto the existing native activation seam. It holds
//! the member session's runtime turn-finalization boundary, applies the
//! shared runtime admission ([`meerkat_runtime::instruction_activation_runtime_admission`]),
//! and asks the session owner to append one keyed activation
//! ([`MobSessionService::activate_instruction_under_runtime_turn_boundary`]).
//! No new journal: the activation is the session owner's durable transcript
//! row, and re-applying the effective activation is a typed `Duplicate`.

use meerkat_core::{
    InstructionActivationAdmissionErrorCode, InstructionActivationDisposition,
    InstructionActivationMutation, InstructionActivationReadPage, InstructionActivationReadQuery,
    InstructionActivationReceipt, InstructionActivationRequest, SessionError,
    SessionServiceHistoryExt as _,
};

use super::MobHandle;
use super::session_service::MobSessionService as _;
use crate::ids::AgentIdentity;

/// Why a member instruction activation did not apply.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MemberInstructionActivationError {
    /// A typed safe-boundary refusal: the member has no materialized session,
    /// is mid-turn, has an open live channel, its current lowering cannot
    /// represent the activation, the session owner has no durable activation
    /// seam, or a fenced store conflicted or asked for backoff. Nothing was
    /// appended.
    #[error("member instruction activation rejected ({code:?}): {message}")]
    Admission {
        code: InstructionActivationAdmissionErrorCode,
        message: String,
    },
    /// The session owner refused the activation itself (for example an
    /// invalid request, a digest mismatch or an immutable-revision conflict).
    #[error(transparent)]
    Session(SessionError),
    /// The runtime authority could not answer.
    #[error("member instruction activation runtime authority failed: {0}")]
    Runtime(#[source] meerkat_runtime::RuntimeDriverError),
    /// The task that owned the activation under the boundary failed.
    #[error("member instruction activation owner task failed: {0}")]
    OwnerTask(String),
}

impl MemberInstructionActivationError {
    /// The typed safe-boundary class, when this is one.
    #[must_use]
    pub const fn admission_code(&self) -> Option<InstructionActivationAdmissionErrorCode> {
        match self {
            Self::Admission { code, .. } => Some(*code),
            Self::Session(_) | Self::Runtime(_) | Self::OwnerTask(_) => None,
        }
    }

    fn not_materialized(identity: &AgentIdentity, detail: &str) -> Self {
        Self::Admission {
            code: InstructionActivationAdmissionErrorCode::TargetNotMaterialized,
            message: format!("member {identity} {detail}"),
        }
    }

    fn from_session_error(error: SessionError) -> Self {
        if let SessionError::NotFound { id } = &error {
            return Self::Admission {
                code: InstructionActivationAdmissionErrorCode::TargetNotMaterialized,
                message: format!("session {id} is not currently materialized"),
            };
        }
        match meerkat_runtime::instruction_activation_admission_for_session_error(&error) {
            Some((code, message)) => Self::Admission { code, message },
            None => Self::Session(error),
        }
    }
}

impl From<meerkat_runtime::InstructionActivationRuntimeRefusal>
    for MemberInstructionActivationError
{
    fn from(refusal: meerkat_runtime::InstructionActivationRuntimeRefusal) -> Self {
        match refusal {
            meerkat_runtime::InstructionActivationRuntimeRefusal::Admission { code, message } => {
                Self::Admission { code, message }
            }
            meerkat_runtime::InstructionActivationRuntimeRefusal::Runtime(error) => {
                Self::Runtime(error)
            }
            other => Self::Runtime(meerkat_runtime::RuntimeDriverError::Internal(
                other.to_string(),
            )),
        }
    }
}

impl MobHandle {
    /// Activate one immutable instruction revision on `identity`'s current
    /// member session at a safe boundary.
    ///
    /// Holds the session's runtime turn-finalization boundary for the whole
    /// admission and append, so the activation cannot interleave with a turn.
    /// The member's next turn sees the activation; a re-apply of the
    /// effective activation returns `Duplicate` without a second row.
    ///
    /// # Errors
    ///
    /// [`MemberInstructionActivationError`]; every `Admission` refusal
    /// appended nothing.
    pub async fn activate_member_instruction(
        &self,
        identity: &AgentIdentity,
        request: InstructionActivationRequest,
    ) -> Result<InstructionActivationReceipt, MemberInstructionActivationError> {
        let session_id = self
            .resolve_bridge_session_id(identity)
            .await
            .ok_or_else(|| {
                MemberInstructionActivationError::not_materialized(
                    identity,
                    "has no session binding",
                )
            })?;
        let runtime_adapter = self.runtime_adapter.clone().ok_or_else(|| {
            MemberInstructionActivationError::Admission {
                code: InstructionActivationAdmissionErrorCode::DurabilityUnavailable,
                message: "this mob has no runtime adapter to admit an instruction activation"
                    .to_string(),
            }
        })?;

        #[cfg(feature = "openai-live")]
        let live_lifecycle_lease = runtime_adapter
            .acquire_live_open_lifecycle_lease(&session_id)
            .await
            .map_err(MemberInstructionActivationError::Runtime)?;
        let turn_boundary = self
            .session_service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await
            .map_err(MemberInstructionActivationError::from_session_error)?;
        if !self
            .session_service
            .live_session_actor_registered(&session_id)
            .await
            .map_err(MemberInstructionActivationError::from_session_error)?
        {
            return Err(MemberInstructionActivationError::not_materialized(
                identity,
                "has no materialized session",
            ));
        }
        meerkat_runtime::instruction_activation_runtime_admission(
            &runtime_adapter,
            &session_id,
            None,
        )
        .await?;

        // The append runs on its own task so a caller that goes away cannot
        // cancel it halfway through the boundary it holds.
        let session_service = std::sync::Arc::clone(&self.session_service);
        let owned_session_id = session_id.clone();
        let mutation = crate::tokio::spawn(async move {
            let result = session_service
                .activate_instruction_under_runtime_turn_boundary(&owned_session_id, request, None)
                .await
                .map_err(MemberInstructionActivationError::from_session_error);
            drop(turn_boundary);
            #[cfg(feature = "openai-live")]
            drop(live_lifecycle_lease);
            result
        })
        .await
        .map_err(|error| MemberInstructionActivationError::OwnerTask(error.to_string()))??;
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

    /// Read the durable instruction activation records of `identity`'s
    /// current member session. A transcript read, not a status cache.
    ///
    /// # Errors
    ///
    /// [`MemberInstructionActivationError`] when the member has no session
    /// binding or the session owner cannot read activations.
    pub async fn read_member_instruction_activations(
        &self,
        identity: &AgentIdentity,
        query: InstructionActivationReadQuery,
    ) -> Result<InstructionActivationReadPage, MemberInstructionActivationError> {
        let session_id = self
            .resolve_bridge_session_id(identity)
            .await
            .ok_or_else(|| {
                MemberInstructionActivationError::not_materialized(
                    identity,
                    "has no session binding",
                )
            })?;
        self.session_service
            .read_instruction_activation_records(&session_id, query)
            .await
            .map_err(MemberInstructionActivationError::from_session_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trait default (a decorator that does not forward the seam) is a
    /// typed refusal, never a silent success.
    #[test]
    fn an_owner_without_the_seam_is_refused_typed() {
        let error = MemberInstructionActivationError::from_session_error(
            SessionError::Unsupported("no seam".to_string()),
        );
        assert_eq!(
            error.admission_code(),
            Some(InstructionActivationAdmissionErrorCode::DurabilityUnavailable)
        );
        let not_live =
            MemberInstructionActivationError::from_session_error(SessionError::NotFound {
                id: meerkat_core::SessionId::new(),
            });
        assert_eq!(
            not_live.admission_code(),
            Some(InstructionActivationAdmissionErrorCode::TargetNotMaterialized)
        );
    }
}
