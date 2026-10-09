//! Hook integration helpers for agent lifecycle and loop points.

use crate::agent::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::hooks::{
    HookBackgroundObservation, HookBackgroundResult, HookBackgroundSkip, HookDecision,
    HookEngineError, HookExecutionReport, HookFailureReason, HookInvocation,
    background_feedback_reason,
};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use tokio::sync::mpsc;

/// Call-local projections only. Concurrent pre-tool hooks return these to the
/// mutable caller; no event await can strand an already-returned notice.
pub(super) struct CollectedHookExecution {
    result: Result<HookExecutionReport, AgentError>,
    notices: Vec<crate::types::Message>,
    events: Vec<AgentEvent>,
}

impl CollectedHookExecution {
    pub(super) fn into_result(self) -> Result<HookExecutionReport, AgentError> {
        debug_assert!(self.notices.is_empty() && self.events.is_empty());
        self.result
    }
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Transfer ready facts for this exact active scope. All model notices are
    /// appended before the first event await, so cancellation of event delivery
    /// cannot strand a destructively transferred completion between notices.
    /// This does not start a model call or claim durable/late delivery.
    pub(super) async fn transfer_background_hook_completions(
        &mut self,
        run_id: &crate::RunId,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        let Some(engine) = &self.hook_engine else {
            return;
        };
        let completions = self
            .post_commit_hooks
            .take_active_background_completions(engine, run_id, 32);
        let mut events = Vec::with_capacity(completions.len());
        for registered in completions {
            let completion = &registered.completion;
            let attribution = &completion.attribution;
            let event = match &completion.result {
                HookBackgroundResult::Completed(outcome) => match &outcome.failure_reason {
                    None => AgentEvent::HookCompleted {
                        hook_id: attribution.hook_id.clone(),
                        point: attribution.point,
                        duration_ms: outcome.duration_ms.unwrap_or(0),
                    },
                    Some(reason) => AgentEvent::HookFailed {
                        hook_id: attribution.hook_id.clone(),
                        point: attribution.point,
                        reason: background_feedback_reason(reason),
                    },
                },
                HookBackgroundResult::LaunchRefused(reason) => AgentEvent::HookLaunchRefused {
                    hook_id: attribution.hook_id.clone(),
                    point: attribution.point,
                    reason: background_feedback_reason(reason),
                    tool_use_id: attribution.tool_use_id.clone(),
                },
                HookBackgroundResult::Failed(reason) => AgentEvent::HookFailed {
                    hook_id: attribution.hook_id.clone(),
                    point: attribution.point,
                    reason: background_feedback_reason(reason),
                },
            };
            self.session.push(crate::types::Message::SystemNotice(
                registered.system_notice_record().notice,
            ));
            events.push(event);
        }
        for event in events {
            crate::event_tap::tap_emit(&self.event_tap, event_tx, event).await;
        }
    }

    pub(super) async fn execute_hooks(
        &mut self,
        invocation: HookInvocation,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<HookExecutionReport, AgentError> {
        let mut collected = self.collect_hook_execution(invocation).await;
        self.append_collected_hook_notices(&mut collected);
        self.emit_collected_hook_events(&mut collected, event_tx)
            .await;
        collected.into_result()
    }

    /// Immutable execution preserves existing concurrent pre-tool dispatch.
    /// Projection is call-local and does not await event delivery.
    pub(super) async fn collect_hook_execution(
        &self,
        mut invocation: HookInvocation,
    ) -> CollectedHookExecution {
        let mut collected = CollectedHookExecution {
            result: Ok(HookExecutionReport::empty()),
            notices: Vec::new(),
            events: Vec::new(),
        };
        let Some(hook_engine) = &self.hook_engine else {
            return collected;
        };
        if invocation.run_id.is_none() {
            invocation.run_id = self.tool_dispatch_context.run_id().cloned();
        }
        collected.result = match hook_engine
            .execute(invocation.clone(), Some(&self.hook_run_overrides))
            .await
        {
            Ok(report) => {
                Self::project_hook_report(
                    &invocation,
                    &report,
                    &mut collected.notices,
                    &mut collected.events,
                );
                Ok(report)
            }
            Err(mut error) => loop {
                match error {
                    HookEngineError::WithReport {
                        report,
                        error: next,
                    } => {
                        Self::project_hook_report(
                            &invocation,
                            &report,
                            &mut collected.notices,
                            &mut collected.events,
                        );
                        error = *next;
                    }
                    error => {
                        Self::project_hook_engine_error(&invocation, &error, &mut collected.events);
                        break Err(error.into_agent_error());
                    }
                }
            },
        };
        collected
    }

    pub(super) fn append_collected_hook_notices(&mut self, collected: &mut CollectedHookExecution) {
        for notice in collected.notices.drain(..) {
            self.session.push(notice);
        }
    }

    pub(super) async fn emit_collected_hook_events(
        &self,
        collected: &mut CollectedHookExecution,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) {
        debug_assert!(
            collected.notices.is_empty(),
            "transfer notices before awaiting events"
        );
        for event in collected.events.drain(..) {
            crate::event_tap::tap_emit(&self.event_tap, event_tx, event).await;
        }
    }

    fn project_hook_report(
        invocation: &HookInvocation,
        report: &HookExecutionReport,
        notices: &mut Vec<crate::types::Message>,
        events: &mut Vec<AgentEvent>,
    ) {
        for skipped in &report.background_skips {
            notices.push(background_scheduling_notice(invocation, skipped));
        }
        // Background scheduling is absent from `started`. Its completion may
        // later establish entry, but scheduling pressure never does.
        for hook_id in &report.started {
            events.push(AgentEvent::HookStarted {
                hook_id: hook_id.clone(),
                point: invocation.point,
            });
        }
        for outcome in &report.outcomes {
            events.push(match &outcome.failure_reason {
                Some(reason) => AgentEvent::HookFailed {
                    hook_id: outcome.hook_id.clone(),
                    point: outcome.point,
                    reason: reason.clone(),
                },
                None => AgentEvent::HookCompleted {
                    hook_id: outcome.hook_id.clone(),
                    point: outcome.point,
                    duration_ms: outcome.duration_ms.unwrap_or(0),
                },
            });
        }
        for refusal in &report.launch_refusals {
            events.push(AgentEvent::HookLaunchRefused {
                hook_id: refusal.hook_id.clone(),
                point: refusal.point,
                reason: HookFailureReason::ConfinementRefused {
                    refusal: refusal.refusal,
                },
                tool_use_id: invocation
                    .tool_call
                    .as_ref()
                    .map(|call| call.tool_use_id.clone())
                    .or_else(|| {
                        invocation
                            .tool_result
                            .as_ref()
                            .map(|result| result.tool_use_id.clone())
                    }),
            });
        }
        if let Some(HookDecision::Deny {
            hook_id,
            reason_code,
            message,
            payload,
        }) = &report.decision
        {
            events.push(AgentEvent::HookDenied {
                hook_id: hook_id.clone(),
                point: invocation.point,
                reason_code: *reason_code,
                message: message.clone(),
                payload: payload.clone(),
            });
        }
    }

    fn project_hook_engine_error(
        invocation: &HookInvocation,
        error: &HookEngineError,
        events: &mut Vec<AgentEvent>,
    ) {
        if let HookEngineError::LaunchRefused { hook_id, reason } = error {
            events.push(AgentEvent::HookLaunchRefused {
                hook_id: hook_id.clone(),
                point: invocation.point,
                reason: reason.clone(),
                tool_use_id: invocation
                    .tool_call
                    .as_ref()
                    .map(|call| call.tool_use_id.clone())
                    .or_else(|| {
                        invocation
                            .tool_result
                            .as_ref()
                            .map(|result| result.tool_use_id.clone())
                    }),
            });
            return;
        }
        if let Some(hook_id) = error.hook_id() {
            // Preserve the existing foreground error classification/observations.
            events.push(AgentEvent::HookStarted {
                hook_id: hook_id.clone(),
                point: invocation.point,
            });
            events.push(AgentEvent::HookFailed {
                hook_id: hook_id.clone(),
                point: invocation.point,
                reason: HookFailureReason::from_engine_error(error),
            });
        }
    }
}

/// Each variable identity is bounded independently for audience projection. An
/// oversized identity is omitted explicitly, never replaced with an exact-looking
/// prefix. This does not change the engine's original typed scheduling report.
fn background_scheduling_notice(
    invocation: &HookInvocation,
    skipped: &HookBackgroundSkip,
) -> crate::types::Message {
    const ID_BYTES: usize = 1024;
    let hook_id = (skipped.hook_id.0.len() <= ID_BYTES).then_some(skipped.hook_id.0.as_str());
    let original_call = invocation
        .tool_call
        .as_ref()
        .map(|call| call.tool_use_id.as_str())
        .or_else(|| {
            invocation
                .tool_result
                .as_ref()
                .map(|result| result.tool_use_id.as_str())
        });
    let tool_use_id = original_call.filter(|id| id.len() <= ID_BYTES);
    let oversized_observation = matches!(invocation.observation.as_ref(),
        Some(crate::hooks::HookObservation::PeerIngressCommitted(value))
            if value.request_id.as_ref().is_some_and(|id| id.len() > ID_BYTES));
    let observation = if oversized_observation {
        None
    } else {
        invocation
            .observation
            .as_ref()
            .map(HookBackgroundObservation::from_observation)
    };
    let attribution_truncated =
        hook_id.is_none() || original_call != tool_use_id || oversized_observation;
    let payload = serde_json::json!({
        "attribution": {
            "session_id": invocation.session_id, "run_id": invocation.run_id,
            "turn_number": invocation.turn_number, "hook_id": hook_id,
            "point": skipped.point, "tool_use_id": tool_use_id, "observation": observation,
        },
        "disposition": "not_scheduled", "reason": skipped.reason,
        "attribution_truncated": attribution_truncated,
    });
    crate::types::Message::SystemNotice(crate::types::SystemNoticeMessage::with_block(
        crate::types::SystemNoticeKind::Generic,
        Some("A background observation hook was not scheduled. No hook target entry is claimed. Continue with permitted work; any oversized attribution fields were explicitly omitted.".to_owned()),
        crate::types::SystemNoticeBlock::RuntimeNotice {
            category: "background_hook_not_scheduled".to_owned(),
            detail: Some(payload.to_string()), payload: Some(payload),
        },
    ))
}

#[cfg(test)]
mod background_notice_tests {
    use super::*;

    #[test]
    fn scheduling_notice_bounds_oversized_identity_without_losing_original_run()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut invocation =
            HookInvocation::new(crate::HookPoint::PreToolExecution, crate::SessionId::new());
        invocation.run_id = Some(crate::RunId::new());
        invocation.tool_call = Some(crate::HookToolCall {
            tool_use_id: "exact-call".to_owned(),
            name: "private-tool-name".to_owned(),
            args: crate::event::ToolCallArguments::empty(),
            provenance: None,
        });
        let skipped = HookBackgroundSkip {
            hook_id: crate::HookId::new("private-oversized-canary".repeat(1024)),
            point: invocation.point,
            reason: crate::hooks::HookBackgroundSkipReason::AttributionTooLarge,
        };
        let crate::types::Message::SystemNotice(notice) =
            background_scheduling_notice(&invocation, &skipped)
        else {
            return Err("expected runtime notice".into());
        };
        let crate::types::SystemNoticeBlock::RuntimeNotice {
            payload: Some(payload),
            ..
        } = &notice.blocks[0]
        else {
            return Err("expected bounded structured metadata".into());
        };
        assert_eq!(
            payload["attribution"]["run_id"],
            serde_json::to_value(&invocation.run_id)?
        );
        assert_eq!(payload["attribution"]["tool_use_id"], "exact-call");
        assert!(payload["attribution"]["hook_id"].is_null());
        assert_eq!(payload["attribution_truncated"], true);
        assert_eq!(payload["disposition"], "not_scheduled");
        assert_eq!(payload["reason"], "attribution_too_large");
        let projection = notice.blocks[0].model_projection_text();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&projection)?,
            *payload
        );
        assert!(projection.len() < 4096);
        assert!(!projection.contains("private-oversized-canary"));
        assert!(!projection.contains("private-tool-name"));
        Ok(())
    }
}
