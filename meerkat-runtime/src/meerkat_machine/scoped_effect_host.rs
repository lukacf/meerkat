use super::{MachineCleanupTaskSpawner, MeerkatMachine};
use crate::RuntimeDriverError;
use crate::live_ledger::completion::{LiveCompletionText, LivePhysicalEffectOutcome};
use meerkat_core::execution_scope::{
    ScopedEffectFeedback, ScopedEffectHost, ScopedEffectOutcome, ScopedEffectSettlement,
    ScopedEffectStartPermit, ScopedEffectTarget, ScopedExecutionContext, ScopedRunAuthority,
};
use meerkat_core::{EvaluatedToolExecutionPolicy, ToolError};
use std::sync::Arc;

impl MeerkatMachine {
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn prepare_next_batch_for_live_scope_authority_test(
        &self,
        session_id: &meerkat_core::SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<
        (
            meerkat_core::RunId,
            meerkat_core::execution_scope::RunExecutionAuthority,
        ),
        RuntimeDriverError,
    > {
        let driver = {
            let sessions = self.sessions.read().await;
            Arc::clone(
                &sessions
                    .get(session_id)
                    .ok_or(RuntimeDriverError::Destroyed)?
                    .driver,
            )
        };
        let batch = {
            let driver = driver.lock().await;
            super::machine_authorize_runtime_loop_batch(&driver)
                .ok_or_else(|| RuntimeDriverError::Internal("no authorized queued batch".into()))?
        };
        if batch.input_ids() != std::slice::from_ref(input_id) {
            return Err(RuntimeDriverError::Internal(
                "fixture queued batch identity mismatch".into(),
            ));
        }
        let run_id = meerkat_core::RunId::new();
        match super::prepare_runtime_loop_batch_start(&driver, run_id.clone(), batch).await? {
            super::driver::RuntimeLoopBatchStart::Started(authority) => Ok((run_id, authority)),
            outcome => Err(RuntimeDriverError::Internal(format!(
                "fixture run did not start: {outcome:?}"
            ))),
        }
    }

    /// Attach the native claim/feedback owner to a sealed scope. Every claim
    /// still checks current durable scope and registered executor authority.
    pub fn scoped_execution_context(
        &self,
        scope: ScopedRunAuthority,
    ) -> Result<ScopedExecutionContext, RuntimeDriverError> {
        Ok(ScopedExecutionContext::new(
            scope,
            self.scoped_effect_host()?,
        ))
    }

    pub async fn scoped_actor_execution_context(
        &self,
        scope: ScopedRunAuthority,
    ) -> Result<ScopedExecutionContext, RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(&scope.record().executor.session_id)
            .ok_or_else(|| RuntimeDriverError::NotFound {
                runtime_id: crate::LogicalRuntimeId::for_session(
                    &scope.record().executor.session_id,
                ),
            })?;
        entry.require_durability_ready().map_err(|required| {
            RuntimeDriverError::RecoveryRepairBlocked {
                evidence_digest: None,
                reason: required.to_string(),
            }
        })?;
        let bindings = entry.canonical_runtime_bindings.as_ref().ok_or_else(|| {
            RuntimeDriverError::RecoveryRepairBlocked {
                evidence_digest: None,
                reason: "scoped actor handoff requires the registered runtime bindings".into(),
            }
        })?;
        ScopedExecutionContext::for_session_actor(scope, self.scoped_effect_host()?, bindings)
            .map_err(|error| RuntimeDriverError::ValidationFailed {
                reason: error.to_string(),
            })
    }

    pub fn scoped_effect_host(&self) -> Result<Arc<dyn ScopedEffectHost>, RuntimeDriverError> {
        if self.store.is_none() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "scoped tool execution requires persistent storage".into(),
            });
        }
        Ok(Arc::new(NativeScopedEffectHost {
            machine: self.clone(),
            cleanup: MachineCleanupTaskSpawner::acquire()?,
        }))
    }
}

struct NativeScopedEffectHost {
    machine: MeerkatMachine,
    cleanup: MachineCleanupTaskSpawner,
}

#[async_trait::async_trait]
impl ScopedEffectHost for NativeScopedEffectHost {
    async fn claim_callback_application(
        &self,
        scope: ScopedRunAuthority,
    ) -> Result<meerkat_core::execution_scope::ScopedCallbackApplicationPermit, ToolError> {
        self.machine
            .claim_live_callback_application(scope)
            .await
            .map_err(|error| ToolError::execution_failed(error.to_string()))
    }

    async fn resolve_model_attempt(
        &self,
        scope: ScopedRunAuthority,
        request_id: meerkat_core::ops::OperationId,
    ) -> Result<meerkat_core::execution_scope::ScopedModelAttemptResolution, ToolError> {
        self.machine
            .live_request_owner_for_session(&scope.record().executor.session_id)
            .await
            .map_err(|error| ToolError::execution_failed(error.to_string()))?
            .resolve_model_attempt(&scope, &request_id)
            .await
            .map_err(|error| ToolError::execution_failed(error.to_string()))
    }

    async fn claim_model_effect(
        &self,
        scope: ScopedRunAuthority,
        effect_id: meerkat_core::ops::OperationId,
        evaluation: meerkat_core::execution_scope::EvaluatedModelRequestPolicy,
    ) -> Result<ScopedEffectStartPermit<ScopedEffectTarget>, ToolError> {
        self.machine
            .claim_live_model_effect(scope, effect_id, evaluation)
            .await
            .map_err(|error| ToolError::execution_failed(error.to_string()))
    }

    async fn claim_tool_effect(
        &self,
        scope: ScopedRunAuthority,
        effect_id: meerkat_core::ops::OperationId,
        evaluation: EvaluatedToolExecutionPolicy,
    ) -> Result<ScopedEffectStartPermit<ScopedEffectTarget>, ToolError> {
        self.machine
            .claim_live_tool_effect(scope, effect_id, evaluation)
            .await
            .map_err(|error| ToolError::execution_failed(error.to_string()))
    }

    fn submit_feedback(&self, feedback: ScopedEffectFeedback) -> ScopedEffectSettlement {
        let machine = self.machine.clone();
        let task = self.cleanup.spawn(async move {
            let result = async {
                let diagnostic = LiveCompletionText::new("scoped physical effect feedback")
                    .map_err(|error| RuntimeDriverError::Internal(format!("{error}")))?;
                match feedback {
                    ScopedEffectFeedback::NotStarted(proof) => {
                        machine
                            .settle_live_effect_not_started(proof, diagnostic)
                            .await
                    }
                    ScopedEffectFeedback::Observed {
                        claim,
                        outcome,
                        token_accounting,
                    } => {
                        let outcome = match outcome {
                            ScopedEffectOutcome::Succeeded => LivePhysicalEffectOutcome::Succeeded,
                            ScopedEffectOutcome::Failed => LivePhysicalEffectOutcome::Failed,
                            ScopedEffectOutcome::Unknown => LivePhysicalEffectOutcome::Unknown,
                        };
                        machine
                            .settle_live_effect_with_token_accounting(
                                claim,
                                outcome,
                                token_accounting,
                                diagnostic,
                            )
                            .await
                    }
                }
            }
            .await;
            if let Err(error) = &result {
                tracing::error!(%error, "scoped effect feedback could not be persisted");
            }
            result
                .map(|_| ())
                .map_err(|error| ToolError::execution_failed(error.to_string()))
        });
        Box::pin(async move {
            task.await.map_err(|error| {
                ToolError::execution_failed(format!("scoped effect feedback task failed: {error}"))
            })?
        })
    }
}
