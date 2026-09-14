use super::{CallbackToolBatchState, PendingCallbackBatchError, PendingCallbackToolBatch, Session};
use crate::SessionId;
use crate::execution_scope::RunEffectScopeId;
use crate::lifecycle::RunId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Exact ordinary callback correlation content, not continuation permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackBatchIdentity {
    session_id: SessionId,
    run_id: RunId,
    execution_scope: Option<RunEffectScopeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_boundary: Option<crate::ops::OperationId>,
    batch_digest: [u8; 32],
}

impl CallbackBatchIdentity {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn execution_scope(&self) -> Option<RunEffectScopeId> {
        self.execution_scope
    }

    pub fn batch_digest(&self) -> &[u8; 32] {
        &self.batch_digest
    }

    /// Maximum byte length when this record is stored as a JSON string.
    /// This measures representation only; it creates no callback or permission.
    pub fn encoded_storage_byte_ceiling() -> Result<usize, serde_json::Error> {
        let id = uuid::Uuid::nil();
        let extremum = Self {
            session_id: SessionId::from_uuid(id),
            run_id: RunId::from_uuid(id),
            execution_scope: Some(RunEffectScopeId::from_uuid(id)),
            execution_boundary: Some(crate::ops::OperationId(id)),
            batch_digest: [u8::MAX; 32],
        };
        serde_json::to_vec(&serde_json::to_string(&extremum)?).map(|bytes| bytes.len())
    }

    /// Candidate effect identity for a call in this batch, not evidence that
    /// the call belongs to it or that the physical effect was claimed.
    pub fn scoped_tool_effect_id(
        &self,
        call_id: &str,
    ) -> Result<Option<crate::ops::OperationId>, CallbackBatchObservationError> {
        match (self.execution_scope, self.execution_boundary.as_ref()) {
            (None, None) => Ok(None),
            (Some(scope), Some(boundary)) => crate::execution_scope::callback_tool_effect_id(
                scope,
                &self.run_id,
                boundary,
                call_id,
            )
            .map(Some)
            .map_err(|error| CallbackBatchObservationError::InvalidBatch(error.to_string())),
            _ => Err(CallbackBatchObservationError::EffectIdentityUnavailable),
        }
    }
}

/// A read of the existing session callback owner, never an execution grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackBatchObservation {
    Pending {
        identity: CallbackBatchIdentity,
        pending_tool_use_ids: Vec<String>,
    },
    Applied {
        identity: CallbackBatchIdentity,
        pending_tool_use_ids: Vec<String>,
        resume_effects_applied: bool,
    },
}

/// Content observed in the session's retained callback and deferred-result
/// records. Neither completeness nor this observation authorizes continuation.
#[derive(Debug, Clone, PartialEq)]
pub enum StagedCallbackResultsObservation {
    Incomplete {
        missing_tool_use_ids: Vec<String>,
    },
    Complete(CompleteCallbackResults),
    AlreadyApplied {
        results_digest: Option<[u8; 32]>,
        resume_effects_applied: bool,
    },
}

/// Exact, ordered callback content produced only by the ordinary session owner.
/// A consumer must separately fence the observed session and obtain generated
/// continuation authority before applying any result or sibling effect.
#[derive(Debug, Clone, PartialEq)]
pub struct CompleteCallbackResults {
    identity: CallbackBatchIdentity,
    ordered_results: Vec<crate::ToolResult>,
    digest: [u8; 32],
}

impl CompleteCallbackResults {
    pub fn identity(&self) -> &CallbackBatchIdentity {
        &self.identity
    }

    pub fn ordered_results(&self) -> &[crate::ToolResult] {
        &self.ordered_results
    }

    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallbackBatchObservationError {
    #[error("callback batch is invalid: {0}")]
    InvalidBatch(String),
    #[error("historical applied callback receipt has no exact batch identity")]
    AppliedIdentityUnavailable,
    #[error("callback batch identity belongs to another session")]
    ForeignSession,
    #[error("callback terminal does not match the pending batch")]
    TerminalMismatch,
    #[error("callback batch has no exact scoped invocation boundary")]
    EffectIdentityUnavailable,
    #[error("callback result observation requires the exact retained batch")]
    TargetMismatch,
    #[error("staged callback results are invalid: {0}")]
    InvalidStagedResults(String),
}

impl PendingCallbackToolBatch {
    pub(crate) fn post_tool_messages(
        &self,
        identity: Option<&crate::types::TranscriptMessageIdentity>,
    ) -> Vec<crate::Message> {
        self.session_effects
            .iter()
            .filter_map(|effect| match effect {
                crate::ops::SessionEffect::AppendAssistantBlocks { blocks } => {
                    let mut message = crate::types::BlockAssistantMessage::new(
                        blocks.clone(),
                        crate::types::StopReason::EndTurn,
                    );
                    message.identity = identity
                        .cloned()
                        .unwrap_or_default()
                        .with_run_id(self.run_id.clone());
                    Some(crate::Message::BlockAssistant(message))
                }
                _ => None,
            })
            .collect()
    }
}

impl Session {
    pub(crate) fn scoped_callback_readiness(
        &self,
        scope: &crate::execution_scope::ScopedRunAuthority,
    ) -> Result<StagedCallbackResultsObservation, PendingCallbackBatchError> {
        let continuation = scope
            .record()
            .callback_continuation
            .as_ref()
            .ok_or(PendingCallbackBatchError::ScopedContinuationRequired)?;
        if &scope.record().executor.session_id != self.id() {
            return Err(PendingCallbackBatchError::ScopedContinuationRequired);
        }
        match self.callback_tool_batch_state()? {
            Some(super::CallbackToolBatchState::Pending { batch, .. })
                if batch.session_effects.iter().any(|effect| {
                    !matches!(
                        effect,
                        crate::ops::SessionEffect::AppendAssistantBlocks { .. }
                    )
                }) || !batch.async_ops.is_empty() =>
            {
                return Err(PendingCallbackBatchError::ScopedCallbackEffectsUnsupported);
            }
            Some(super::CallbackToolBatchState::Applied { async_ops, .. })
                if !async_ops.is_empty() =>
            {
                return Err(PendingCallbackBatchError::ScopedCallbackEffectsUnsupported);
            }
            _ => {}
        }
        let observed = self
            .observe_staged_callback_results(&continuation.target)
            .map_err(|error| PendingCallbackBatchError::Malformed(error.to_string()))?;
        let digest = match &observed {
            StagedCallbackResultsObservation::Complete(complete) => Some(*complete.digest()),
            StagedCallbackResultsObservation::AlreadyApplied { results_digest, .. } => {
                *results_digest
            }
            StagedCallbackResultsObservation::Incomplete { .. } => None,
        };
        if digest != Some(continuation.results_digest) {
            return Err(PendingCallbackBatchError::ConflictingRedelivery);
        }
        Ok(observed)
    }

    pub(crate) fn apply_scoped_callback_tool_results(
        &mut self,
        permit: crate::execution_scope::ScopedCallbackApplicationPermit,
        identity: Option<&crate::types::TranscriptMessageIdentity>,
    ) -> Result<(), PendingCallbackBatchError> {
        let StagedCallbackResultsObservation::Complete(complete) =
            self.scoped_callback_readiness(permit.scope())?
        else {
            return Err(PendingCallbackBatchError::ConflictingRedelivery);
        };
        let batch = self
            .pending_callback_tool_batch()?
            .ok_or(PendingCallbackBatchError::Missing)?;
        let post_tool_messages = batch.post_tool_messages(identity);
        self.commit_callback_tool_results(&batch, complete.ordered_results, post_tool_messages)
    }

    pub(crate) fn apply_scoped_callback_resume_effects(
        &mut self,
        scope: &crate::execution_scope::ScopedRunAuthority,
    ) -> Result<Vec<crate::event::AssistantImageEvent>, PendingCallbackBatchError> {
        if !matches!(
            self.scoped_callback_readiness(scope)?,
            StagedCallbackResultsObservation::AlreadyApplied { .. }
        ) {
            return Err(PendingCallbackBatchError::ScopedContinuationRequired);
        }
        self.apply_callback_resume_effects()
    }

    /// Read retained result readiness without consuming deferred data, applying
    /// callback results, or inferring currentness from a detached session.
    pub fn observe_staged_callback_results(
        &self,
        target: &CallbackBatchIdentity,
    ) -> Result<StagedCallbackResultsObservation, CallbackBatchObservationError> {
        let invalid = |error: PendingCallbackBatchError| {
            CallbackBatchObservationError::InvalidBatch(error.to_string())
        };
        let Some(state) = self.callback_tool_batch_state().map_err(invalid)? else {
            return Err(CallbackBatchObservationError::TargetMismatch);
        };
        let batch = match state {
            CallbackToolBatchState::Pending { batch, identity } => {
                let identity = self
                    .validate_pending_callback_identity(&batch, identity.as_ref())
                    .map_err(invalid)?;
                if &identity != target {
                    return Err(CallbackBatchObservationError::TargetMismatch);
                }
                batch
            }
            CallbackToolBatchState::Applied {
                identity,
                complete_results_digest,
                results,
                post_tool_messages_applied,
                ..
            } => {
                if identity.as_ref() != Some(target) {
                    return Err(CallbackBatchObservationError::TargetMismatch);
                }
                self.classify_callback_ingress(&results).map_err(invalid)?;
                return Ok(StagedCallbackResultsObservation::AlreadyApplied {
                    results_digest: complete_results_digest,
                    resume_effects_applied: post_tool_messages_applied,
                });
            }
        };
        let deferred = self.try_deferred_turn_state().map_err(|error| {
            CallbackBatchObservationError::InvalidStagedResults(error.to_string())
        })?;
        let mut incoming = Vec::new();
        if let Some(deferred) = deferred {
            for message in deferred.pending_tool_results() {
                if message.callback_identity.as_ref() != Some(target) {
                    return Err(CallbackBatchObservationError::TargetMismatch);
                }
                self.classify_callback_ingress(&message.results)
                    .map_err(invalid)?;
                incoming.extend(message.results.iter().cloned());
            }
        }
        let by_id = super::unique_tool_results(incoming).map_err(invalid)?;
        let missing_tool_use_ids: Vec<_> = batch
            .pending_tool_use_ids
            .iter()
            .filter(|id| !by_id.contains_key(*id))
            .cloned()
            .collect();
        if !missing_tool_use_ids.is_empty() {
            return Ok(StagedCallbackResultsObservation::Incomplete {
                missing_tool_use_ids,
            });
        }
        let super::ResolvedPendingCallbackToolResults::Pending {
            ordered_results, ..
        } = self
            .resolve_pending_callback_tool_results(by_id.into_values().collect())
            .map_err(invalid)?
        else {
            return Err(CallbackBatchObservationError::TargetMismatch);
        };
        let digest = Self::callback_results_digest(target, &ordered_results)?;
        Ok(StagedCallbackResultsObservation::Complete(
            CompleteCallbackResults {
                identity: target.clone(),
                ordered_results,
                digest,
            },
        ))
    }

    pub(super) fn callback_results_digest(
        target: &CallbackBatchIdentity,
        ordered_results: &[crate::ToolResult],
    ) -> Result<[u8; 32], CallbackBatchObservationError> {
        let encoded = serde_json::to_vec(&(
            "meerkat/complete-callback-results/v1",
            target,
            ordered_results,
        ))
        .map_err(|error| CallbackBatchObservationError::InvalidStagedResults(error.to_string()))?;
        Ok(Sha256::digest(encoded).into())
    }

    /// Capture callback correlation while the terminal producer still owns
    /// this session. Absence stays unattributed for custom/older producers.
    pub fn callback_identity_for_terminal(
        &self,
        terminal: &crate::AgentError,
    ) -> Result<Option<CallbackBatchIdentity>, CallbackBatchObservationError> {
        if !matches!(
            terminal,
            crate::AgentError::CallbackPending { .. }
                | crate::AgentError::CallbackBatchPending { .. }
        ) {
            return Ok(None);
        }
        let Some(observation) = self.callback_batch_observation()? else {
            return Ok(None);
        };
        let CallbackBatchObservation::Pending {
            identity,
            pending_tool_use_ids,
        } = observation
        else {
            return Err(CallbackBatchObservationError::TerminalMismatch);
        };
        let matches = match terminal {
            crate::AgentError::CallbackPending { tool_use_id, .. } => {
                pending_tool_use_ids.as_slice() == std::slice::from_ref(tool_use_id)
            }
            crate::AgentError::CallbackBatchPending { pending_tool_calls } => pending_tool_use_ids
                .iter()
                .map(String::as_str)
                .eq(pending_tool_calls
                    .iter()
                    .map(|call| call.tool_use_id.as_str())),
            _ => false,
        };
        if !matches {
            return Err(CallbackBatchObservationError::TerminalMismatch);
        }
        Ok(Some(identity))
    }

    /// Observe exact callback correlation through its owner instead of parsing
    /// private session metadata. Historical applied receipts remain explicitly
    /// unattributed; they are never assigned the session's latest run.
    pub fn callback_batch_observation(
        &self,
    ) -> Result<Option<CallbackBatchObservation>, CallbackBatchObservationError> {
        let invalid = |error: PendingCallbackBatchError| {
            CallbackBatchObservationError::InvalidBatch(error.to_string())
        };
        match self.callback_tool_batch_state().map_err(invalid)? {
            None => Ok(None),
            Some(CallbackToolBatchState::Pending { batch, identity }) => {
                let identity = self
                    .validate_pending_callback_identity(&batch, identity.as_ref())
                    .map_err(invalid)?;
                Ok(Some(CallbackBatchObservation::Pending {
                    identity,
                    pending_tool_use_ids: batch.pending_tool_use_ids,
                }))
            }
            Some(CallbackToolBatchState::Applied {
                identity,
                tool_use_order,
                post_tool_messages_applied,
                ..
            }) => {
                let identity =
                    identity.ok_or(CallbackBatchObservationError::AppliedIdentityUnavailable)?;
                if identity.session_id() != self.id() {
                    return Err(CallbackBatchObservationError::ForeignSession);
                }
                Ok(Some(CallbackBatchObservation::Applied {
                    identity,
                    pending_tool_use_ids: tool_use_order,
                    resume_effects_applied: post_tool_messages_applied,
                }))
            }
        }
    }

    pub(super) fn pending_callback_identity(
        &self,
        batch: &PendingCallbackToolBatch,
    ) -> Result<CallbackBatchIdentity, PendingCallbackBatchError> {
        super::validate_pending_callback_batch(self.messages(), batch)?;
        let encoded = serde_json::to_vec(&(
            "meerkat/callback-batch/v1",
            self.id(),
            batch,
            self.messages().last(),
        ))
        .map_err(|error| PendingCallbackBatchError::Malformed(error.to_string()))?;
        Ok(CallbackBatchIdentity {
            session_id: self.id().clone(),
            run_id: batch.run_id.clone(),
            execution_scope: batch.execution_scope,
            execution_boundary: batch.execution_boundary.clone(),
            batch_digest: Sha256::digest(encoded).into(),
        })
    }

    pub(super) fn validate_pending_callback_identity(
        &self,
        batch: &PendingCallbackToolBatch,
        retained: Option<&CallbackBatchIdentity>,
    ) -> Result<CallbackBatchIdentity, PendingCallbackBatchError> {
        let identity = self.pending_callback_identity(batch)?;
        if retained.is_some_and(|retained| retained != &identity) {
            return Err(PendingCallbackBatchError::Malformed(
                "retained callback identity differs from its actual batch and assistant tail"
                    .into(),
            ));
        }
        Ok(identity)
    }

    pub(crate) fn require_session_policy_callback_application(
        &self,
    ) -> Result<(), PendingCallbackBatchError> {
        let scope = match self.callback_tool_batch_state()? {
            Some(CallbackToolBatchState::Pending { batch, .. }) => batch.execution_scope,
            Some(CallbackToolBatchState::Applied { identity, .. }) => {
                identity.and_then(|identity| identity.execution_scope())
            }
            None => None,
        };
        if scope.is_some() {
            return Err(PendingCallbackBatchError::ScopedContinuationRequired);
        }
        Ok(())
    }

    pub(crate) fn require_session_policy_callback_continuation(
        &self,
    ) -> Result<(), PendingCallbackBatchError> {
        match self.callback_tool_batch_state()? {
            Some(CallbackToolBatchState::Applied {
                post_tool_messages_applied: true,
                ..
            })
            | None => Ok(()),
            Some(CallbackToolBatchState::Pending { batch, .. })
                if batch.execution_scope.is_none() =>
            {
                Ok(())
            }
            Some(CallbackToolBatchState::Applied { identity, .. })
                if identity
                    .as_ref()
                    .is_none_or(|identity| identity.execution_scope().is_none()) =>
            {
                Ok(())
            }
            Some(
                CallbackToolBatchState::Pending { .. } | CallbackToolBatchState::Applied { .. },
            ) => Err(PendingCallbackBatchError::ScopedContinuationRequired),
        }
    }
}

#[cfg(test)]
#[path = "callback_identity_tests.rs"]
mod tests;
