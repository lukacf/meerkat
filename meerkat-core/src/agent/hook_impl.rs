//! Hook integration helpers for agent lifecycle and loop points.

use crate::agent::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::hooks::{
    HookDecision, HookEngineError, HookExecutionReport, HookFailureReason, HookInvocation,
};
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

        let report = match hook_engine
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

        // `HookStarted` means execution actually began. The engine reports the
        // hook ids it truly started in `report.started` — foreground entries it
        // ran (a deny short-circuit leaves later entries absent) and background
        // entries it acquired a permit for and spawned (a saturated queue leaves
        // skipped entries absent). Emitting from `matching_hooks()` instead would
        // fire `HookStarted` for hooks that never began.
        for hook_id in &report.started {
            crate::event_tap::tap_emit(
                &self.event_tap,
                event_tx,
                AgentEvent::HookStarted {
                    hook_id: hook_id.clone(),
                    point: invocation.point,
                },
            )
            .await;
        }

        for outcome in &report.outcomes {
            if let Some(reason) = &outcome.failure_reason {
                crate::event_tap::tap_emit(
                    &self.event_tap,
                    event_tx,
                    AgentEvent::HookFailed {
                        hook_id: outcome.hook_id.clone(),
                        point: outcome.point,
                        reason: reason.clone(),
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
                    reason: HookFailureReason::from_engine_error(err),
                },
            )
            .await;
        }
    }
}
