//! Runtime-side admission for a safe-boundary instruction activation.
//!
//! One owner for the policy every activation host applies after it holds the
//! session's runtime turn-finalization boundary and has seen the session
//! materialized: the facade session runtime and mob members both call
//! [`instruction_activation_runtime_admission`], so the two hosts cannot
//! drift. This module owns no activation lifecycle; the activation itself is
//! still the session owner's turn-boundary mutation.

use meerkat_core::{InstructionActivationAdmissionErrorCode, SessionError, SessionId};

use crate::meerkat_machine::MeerkatMachine;
use crate::{RuntimeDriverError, RuntimeState, SessionServiceRuntimeExt as _};

/// Why the runtime refused an instruction activation before the session owner
/// was asked to mutate anything.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InstructionActivationRuntimeRefusal {
    /// A typed safe-boundary refusal.
    #[error("instruction activation admission rejected ({code:?}): {message}")]
    Admission {
        code: InstructionActivationAdmissionErrorCode,
        message: String,
    },
    /// The runtime authority could not answer.
    #[error("instruction activation runtime authority failed: {0}")]
    Runtime(#[source] RuntimeDriverError),
}

/// Admit an instruction activation on `session_id` at the runtime boundary:
/// no open live channel, no active runtime work (the generated
/// transcript-edit admission), and a current lowering that can represent an
/// ordered mid-conversation System activation.
///
/// The caller must already hold the session's runtime turn-finalization
/// boundary and have confirmed the session is materialized under it. `model`
/// only names the model in the lowering refusal.
///
/// # Errors
///
/// [`InstructionActivationRuntimeRefusal`] when the activation is not
/// admissible now.
pub async fn instruction_activation_runtime_admission(
    runtime_adapter: &MeerkatMachine,
    session_id: &SessionId,
    model: Option<&str>,
) -> Result<(), InstructionActivationRuntimeRefusal> {
    #[cfg(feature = "live")]
    if runtime_adapter
        .live_active_channel_for_session(session_id)
        .await
        .is_some()
    {
        return Err(InstructionActivationRuntimeRefusal::Admission {
            code: InstructionActivationAdmissionErrorCode::LiveChannelOpen,
            message: format!("session {session_id} has an open live channel"),
        });
    }

    let runtime_running = runtime_adapter
        .runtime_state(session_id)
        .await
        .is_ok_and(|state| matches!(state, RuntimeState::Running));
    let has_active_inputs = runtime_adapter
        .list_active_inputs(session_id)
        .await
        .is_ok_and(|inputs| !inputs.is_empty());
    if !matches!(
        runtime_adapter
            .resolve_transcript_edit_admission(session_id, runtime_running, has_active_inputs)
            .await,
        Ok(crate::meerkat_machine::dsl::TranscriptEditAdmissionKind::Admissible)
    ) {
        return Err(InstructionActivationRuntimeRefusal::Admission {
            code: InstructionActivationAdmissionErrorCode::SessionBusy,
            message: format!("session {session_id} has active runtime work"),
        });
    }

    let resolved_capabilities = runtime_adapter
        .resolved_session_llm_capabilities(session_id)
        .await
        .map_err(InstructionActivationRuntimeRefusal::Runtime)?
        .ok_or_else(|| {
            InstructionActivationRuntimeRefusal::Runtime(RuntimeDriverError::Internal(format!(
                "materialized session {session_id} has no machine-owned resolved llm capability surface"
            )))
        })?;
    if !resolved_capabilities.supports_mid_conversation_system_messages {
        return Err(InstructionActivationRuntimeRefusal::Admission {
            code: InstructionActivationAdmissionErrorCode::UnsupportedCurrentLowering,
            message: format!(
                "model {} for session {session_id} cannot exactly represent an ordered mid-conversation System activation",
                model.unwrap_or("(current)")
            ),
        });
    }
    Ok(())
}

/// The typed safe-boundary class of a session-owner failure from the
/// activation mutation, when it has one: a fenced store's conflict or
/// backoff, and an owner without a durable activation seam
/// (`SessionError::Unsupported`, classified as durability unavailable).
/// Shared by every activation host so the classes are assigned once.
#[must_use]
pub fn instruction_activation_admission_for_session_error(
    error: &SessionError,
) -> Option<(InstructionActivationAdmissionErrorCode, String)> {
    match error {
        SessionError::ExternalWriteFenceConflict { reason } => Some((
            InstructionActivationAdmissionErrorCode::ExternalWriteFenceConflict,
            reason.clone(),
        )),
        SessionError::ExternalWriteFenceBackoff { reason } => Some((
            InstructionActivationAdmissionErrorCode::ExternalWriteFenceBackoff,
            reason.clone(),
        )),
        SessionError::Unsupported(message) => Some((
            InstructionActivationAdmissionErrorCode::DurabilityUnavailable,
            message.clone(),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_without_the_seam_is_typed_durability_unavailable() {
        let classified = instruction_activation_admission_for_session_error(
            &SessionError::Unsupported("no seam".to_string()),
        );
        assert_eq!(
            classified.map(|(code, _)| code),
            Some(InstructionActivationAdmissionErrorCode::DurabilityUnavailable)
        );
        assert!(
            instruction_activation_admission_for_session_error(&SessionError::NotFound {
                id: SessionId::new(),
            })
            .is_none()
        );
    }
}
