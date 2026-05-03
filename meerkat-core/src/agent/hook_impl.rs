//! Hook integration helpers for agent lifecycle and loop points.

use crate::agent::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::hooks::{HookDecision, HookEngineError, HookExecutionReport, HookInvocation};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
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

        {
            let planned = hook_engine
                .matching_hooks(&invocation, Some(&self.hook_run_overrides))
                .map_err(Self::map_hook_engine_error)?;
            for hook_id in planned {
                crate::event_tap::tap_emit(
                    &self.event_tap,
                    event_tx,
                    AgentEvent::HookStarted {
                        hook_id,
                        point: invocation.point,
                    },
                )
                .await;
            }
        }

        let mut report = match hook_engine
            .execute(invocation.clone(), Some(&self.hook_run_overrides))
            .await
        {
            Ok(report) => report,
            Err(err) => {
                self.emit_hook_engine_error(&invocation, event_tx, &err)
                    .await;
                return Err(Self::map_hook_engine_error(err));
            }
        };
        let mut published = match hook_engine
            .drain_published_patches(&invocation.session_id)
            .await
        {
            Ok(published) => published,
            Err(err) => {
                self.emit_hook_engine_error(&invocation, event_tx, &err)
                    .await;
                return Err(Self::map_hook_engine_error(err));
            }
        };
        report.published_patches.append(&mut published);

        for outcome in &report.outcomes {
            if let Some(error) = &outcome.error {
                crate::event_tap::tap_emit(
                    &self.event_tap,
                    event_tx,
                    AgentEvent::HookFailed {
                        hook_id: outcome.hook_id.clone(),
                        point: outcome.point,
                        error: error.clone(),
                    },
                )
                .await;
            } else {
                crate::event_tap::tap_emit(
                    &self.event_tap,
                    event_tx,
                    AgentEvent::HookCompleted {
                        hook_id: outcome.hook_id.clone(),
                        point: outcome.point,
                        duration_ms: outcome.duration_ms.unwrap_or(0),
                    },
                )
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
            crate::event_tap::tap_emit(
                &self.event_tap,
                event_tx,
                AgentEvent::HookDenied {
                    hook_id: hook_id.clone(),
                    point: invocation.point,
                    reason_code: *reason_code,
                    message: message.clone(),
                    payload: payload.clone(),
                },
            )
            .await;
        }

        for envelope in &report.published_patches {
            crate::event_tap::tap_emit(
                &self.event_tap,
                event_tx,
                AgentEvent::HookPatchPublished {
                    hook_id: envelope.hook_id.clone(),
                    point: envelope.point,
                    envelope: envelope.clone(),
                },
            )
            .await;
        }

        Ok(report)
    }

    fn map_hook_engine_error(err: HookEngineError) -> AgentError {
        err.into_agent_error()
    }

    async fn emit_hook_engine_error(
        &self,
        invocation: &HookInvocation,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
        err: &HookEngineError,
    ) {
        if let Some(hook_id) = err.hook_id() {
            crate::event_tap::tap_emit(
                &self.event_tap,
                event_tx,
                AgentEvent::HookFailed {
                    hook_id: hook_id.clone(),
                    point: invocation.point,
                    error: err.to_string(),
                },
            )
            .await;
        }
    }
}
