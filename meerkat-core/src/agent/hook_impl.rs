//! Hook integration helpers for agent lifecycle and loop points.

use crate::agent::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::hooks::{HookDecision, HookEngineError, HookExecutionReport, HookInvocation};
use tokio::sync::mpsc;

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    pub(super) async fn execute_hooks(
        &self,
        invocation: HookInvocation,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<HookExecutionReport, AgentError> {
        let Some(hook_engine) = &self.hook_engine else {
            return Ok(HookExecutionReport::empty());
        };

        let report = hook_engine
            .execute(invocation.clone(), Some(&self.hook_run_overrides))
            .await
            .map_err(Self::map_hook_engine_error)?;

        if let Some(tx) = event_tx {
            for outcome in &report.outcomes {
                let _ = tx
                    .send(AgentEvent::HookStarted {
                        hook_id: outcome.hook_id.to_string(),
                        point: outcome.point,
                    })
                    .await;

                if let Some(error) = &outcome.error {
                    let _ = tx
                        .send(AgentEvent::HookFailed {
                            hook_id: outcome.hook_id.to_string(),
                            point: outcome.point,
                            error: error.clone(),
                        })
                        .await;
                } else {
                    let _ = tx
                        .send(AgentEvent::HookCompleted {
                            hook_id: outcome.hook_id.to_string(),
                            point: outcome.point,
                            duration_ms: outcome.duration_ms.unwrap_or(0),
                        })
                        .await;
                }
            }

            if let Some(HookDecision::Deny {
                hook_id,
                reason_code,
                message,
                payload,
            }) = &report.decision
            {
                let _ = tx
                    .send(AgentEvent::HookDenied {
                        hook_id: hook_id.to_string(),
                        point: invocation.point,
                        reason_code: *reason_code,
                        message: message.clone(),
                        payload: payload.clone(),
                    })
                    .await;
            }

            for envelope in &report.published_patches {
                let _ = tx
                    .send(AgentEvent::HookPatchPublished {
                        hook_id: envelope.hook_id.to_string(),
                        point: envelope.point,
                        envelope: envelope.clone(),
                    })
                    .await;
            }
        }

        Ok(report)
    }

    fn map_hook_engine_error(err: HookEngineError) -> AgentError {
        match err {
            HookEngineError::InvalidConfiguration(reason) => {
                AgentError::HookConfigInvalid { reason }
            }
            HookEngineError::Timeout {
                hook_id,
                timeout_ms,
            } => AgentError::HookTimeout {
                hook_id: hook_id.to_string(),
                timeout_ms,
            },
            HookEngineError::ExecutionFailed { hook_id, reason } => {
                AgentError::HookExecutionFailed {
                    hook_id: hook_id.to_string(),
                    reason,
                }
            }
        }
    }
}
