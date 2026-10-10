//! Owned app IO with canonical session mutation returned to the exact Agent.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use serde_json::value::to_raw_value;

use super::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use crate::error::{AgentError, ToolError};
use crate::hooks::{HookExecutionReport, HookInvocation, HookPoint};
use crate::ops::{ToolDispatchOutcome, ToolDispatchTimeoutPolicy};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_application::{
    ToolApplicationExecution, ToolApplicationExecutionOutcome, ToolApplicationExecutionOwner,
    ToolApplicationExecutor, ToolApplicationRequest, ToolApplicationResolution,
    ToolApplicationSettlement,
};
use crate::tool_execution::{
    ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner, ToolExecutionResolutionContext,
    ToolExecutionResolutionError,
};
use crate::types::ToolCallView;

struct AppExecution<T: AgentToolDispatcher + ?Sized> {
    tools: Arc<T>,
    scope: crate::tool_scope::ToolScope,
    review: Option<Arc<crate::approval::review::BoundOperationReview>>,
    hooks: Option<Arc<dyn crate::hooks::HookEngine>>,
    tap: crate::event_tap::EventTap,
    owner: ToolApplicationExecutionOwner,
    session_id: crate::SessionId,
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Mint execution from this Agent's private accepted invocation owner.
    /// Display readers and caller-supplied result metadata cannot mint it.
    pub fn tool_application_executor(&self) -> ToolApplicationExecutor {
        self.tool_application_observations
            .refresh(self.session.messages());
        let owner = self.tool_application_observations.execution_owner();
        ToolApplicationExecutor {
            execution: Arc::new(AppExecution {
                tools: self.tools.clone(),
                scope: self.tool_scope.clone(),
                review: self.operation_review.clone(),
                hooks: self.hook_engine.clone(),
                tap: self.event_tap.clone(),
                owner: owner.clone(),
                session_id: self.session.id().clone(),
            }),
            owner,
        }
    }

    /// Direct embedded hosts use the same execution and settlement path.
    pub async fn tool_application(
        &mut self,
        request: ToolApplicationRequest,
        fresh_context: crate::ToolDispatchContext,
    ) -> Result<Value, AgentError> {
        let outcome = self
            .tool_application_executor()
            .execute(request, fresh_context)
            .await;
        self.settle_tool_application(outcome).into_parts().0
    }

    /// Apply only an outcome minted by this exact Agent. Dirty evidence is set
    /// before mutation so a partial settlement error still requires persistence.
    pub fn settle_tool_application(
        &mut self,
        mut outcome: ToolApplicationExecutionOutcome,
    ) -> ToolApplicationSettlement {
        let failures = outcome.settlement_failures();
        if !self
            .tool_application_observations
            .execution_owner()
            .same_owner(&outcome.owner)
        {
            return ToolApplicationSettlement {
                result: Err(AgentError::ConfigError(
                    "tool application outcome belongs to another Agent".into(),
                )
                .with_settlement_failures(failures)),
                dirty: false,
            };
        }
        let dirty = outcome.requires_settlement();
        for notice in outcome.notices.drain(..) {
            self.session.push(notice);
        }
        for event in outcome.events.drain(..) {
            crate::event_tap::tap_try_send(&self.event_tap, &event);
        }
        if !outcome.effects.is_empty()
            && let Err(error) = self.apply_session_effects(&outcome.effects, None)
        {
            outcome.result = Err(error.with_settlement_failures(failures));
        }
        if dirty {
            self.tool_application_observations
                .refresh(self.session.messages());
        }
        ToolApplicationSettlement {
            result: outcome.result,
            dirty,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<T: AgentToolDispatcher + ?Sized + 'static> ToolApplicationExecution for AppExecution<T> {
    async fn execute(
        &self,
        request: ToolApplicationRequest,
        context: crate::ToolDispatchContext,
    ) -> ToolApplicationExecutionOutcome {
        let mut outcome = ToolApplicationExecutionOutcome {
            owner: self.owner.clone(),
            result: Err(AgentError::InternalError(
                "tool application execution did not complete".into(),
            )),
            notices: Vec::new(),
            events: Vec::new(),
            effects: Vec::new(),
            settlement_failures: Vec::new(),
        };
        outcome.result = self.execute_inner(request, context, &mut outcome).await;
        outcome
    }
}

impl<T: AgentToolDispatcher + ?Sized + 'static> AppExecution<T> {
    async fn execute_inner(
        &self,
        request: ToolApplicationRequest,
        fresh_context: crate::ToolDispatchContext,
        pending: &mut ToolApplicationExecutionOutcome,
    ) -> Result<Value, AgentError> {
        request
            .validate()
            .map_err(|error| AgentError::tool(error.into()))?;
        let control = fresh_context
            .tool_application_control()
            .ok_or_else(|| AgentError::tool(ToolError::access_denied(&request.tool_call_id)))?
            .clone();
        if control.request() != &request || control.session_id() != &self.session_id {
            return Err(AgentError::tool(ToolError::access_denied(
                &request.tool_call_id,
            )));
        }
        control
            .revalidate_async()
            .await
            .map_err(|error| AgentError::tool(error.into()))?;
        control
            .claim_execution()
            .map_err(|error| AgentError::tool(error.into()))?;
        let origin = self
            .owner
            .accept(&request, self.scope.clone())
            .map_err(|error| AgentError::tool(error.into()))?;
        control
            .bind_execution_guard(origin.clone())
            .map_err(|error| AgentError::tool(error.into()))?;
        let context = fresh_context.with_operation_review(self.review.clone());
        let resolution = self
            .tools
            .resolve_tool_application(
                origin.source(),
                &request,
                origin.invocation(&request.extension),
                &context,
            )
            .await
            .map_err(AgentError::tool)?;
        control
            .revalidate_async()
            .await
            .map_err(|error| AgentError::tool(error.into()))?;
        match resolution {
            ToolApplicationResolution::Value(value) => Ok(value),
            ToolApplicationResolution::Call {
                name,
                binding,
                project_result,
            } => {
                origin
                    .bind_target(name.clone())
                    .map_err(|error| AgentError::tool(error.into()))?;
                let crate::ToolApplicationOperation::CallTool { arguments, .. } = request.operation
                else {
                    return Err(AgentError::tool(ToolError::access_denied(name)));
                };
                let call = crate::types::ToolCall {
                    id: format!("app-{}", uuid::Uuid::new_v4()),
                    name,
                    args: arguments,
                };
                let outcome = self
                    .dispatch_call(call, context.with_application_binding(binding), pending)
                    .await?;
                if let Some(crate::ops::ToolDispatchTerminalCause::RuntimeToolError { error }) =
                    outcome.terminal_cause()
                {
                    return Err(AgentError::tool(
                        error
                            .clone()
                            .with_settlement_failures(outcome.settlement_failures().to_vec()),
                    ));
                }
                project_result(&outcome.result).map_err(|error| {
                    AgentError::tool(
                        error.with_settlement_failures(outcome.settlement_failures().to_vec()),
                    )
                })
            }
        }
    }

    async fn hooks(
        &self,
        invocation: HookInvocation,
        pending: &mut ToolApplicationExecutionOutcome,
    ) -> Result<HookExecutionReport, AgentError> {
        // Configured hooks only. A prior model run's overrides and attribution
        // never become authority for a fresh app action.
        let mut collected = super::hook_impl::collect_hook_execution_with_engine(
            self.hooks.as_ref(),
            invocation,
            None,
        )
        .await;
        pending.notices.append(&mut collected.notices);
        if pending.notices.is_empty() {
            for event in collected.events.drain(..) {
                crate::event_tap::tap_emit(&self.tap, None, event).await;
            }
        } else {
            pending.events.append(&mut collected.events);
        }
        collected.into_result()
    }

    async fn dispatch_call(
        &self,
        call: crate::types::ToolCall,
        context: crate::ToolDispatchContext,
        pending: &mut ToolApplicationExecutionOutcome,
    ) -> Result<ToolDispatchOutcome, AgentError> {
        let provenance = self
            .scope
            .app_visible_tools_result()
            .map_err(|error| AgentError::InternalError(error.to_string()))?
            .iter()
            .find(|tool| tool.name.as_str() == call.name)
            .and_then(|tool| tool.provenance.clone());
        let mut invocation = HookInvocation {
            point: HookPoint::PreToolExecution,
            session_id: self.session_id.clone(),
            run_id: None,
            turn_number: None,
            prompt_input: None,
            error_report: None,
            error_class: None,
            llm_request: None,
            llm_response: None,
            tool_call: Some(crate::hooks::HookToolCall {
                tool_use_id: call.id.clone(),
                name: call.name.clone(),
                args: crate::ToolCallArguments::from_value(call.args.clone()).map_err(|error| {
                    AgentError::tool(ToolError::invalid_arguments(&call.name, error.to_string()))
                })?,
                provenance: provenance.clone(),
            }),
            tool_result: None,
            observation: None,
        };
        let report = self.hooks(invocation.clone(), pending).await?;
        if let Some(denial) = report.denial(HookPoint::PreToolExecution) {
            return Err(AgentError::tool(ToolError::HookDenied {
                denial: Box::new(denial),
            }));
        }
        let control = context
            .tool_application_control()
            .ok_or_else(|| AgentError::tool(ToolError::access_denied(&call.name)))?
            .clone();
        control
            .revalidate_async()
            .await
            .map_err(|error| AgentError::tool(error.into()))?;
        let mut outcome = dispatch_host_tool_call(
            &self.tools,
            call.clone(),
            ToolDispatchTimeoutPolicy::Disabled,
            context,
        )
        .await?;
        // Transfer effects immediately, before any post-hook await can fail.
        pending.effects.append(&mut outcome.session_effects);
        pending
            .settlement_failures
            .extend_from_slice(outcome.settlement_failures());
        invocation.point = HookPoint::PostToolExecution;
        invocation.tool_call = None;
        invocation.tool_result = Some(
            crate::hooks::HookToolResult::from_tool_result_with_id(
                call.id,
                call.name,
                &outcome.result,
            )
            .with_provenance(provenance),
        );
        let publication = self.hooks(invocation, pending).await;
        let mut publication = match publication {
            Ok(report) => match report.denial(HookPoint::PostToolExecution) {
                Some(denial) => Err(AgentError::tool(ToolError::HookDenied {
                    denial: Box::new(crate::hooks::HookDenial {
                        message: "Tool result publication was withheld; this does not undo any entered tool execution.".into(),
                        payload: None, ..denial
                    }),
                }.with_settlement_failures(outcome.settlement_failures().to_vec()))),
                None => Ok(()),
            },
            Err(error) => Err(error.with_settlement_failures(outcome.settlement_failures().to_vec())),
        };
        if publication.is_ok() {
            publication = control.revalidate_async().await.map_err(|error| {
                AgentError::tool(error.into())
                    .with_settlement_failures(outcome.settlement_failures().to_vec())
            });
        }
        if publication.is_err() {
            pending.effects.retain(|effect| {
                !matches!(
                    effect,
                    crate::ops::SessionEffect::AppendAssistantBlocks { .. }
                )
            });
        }
        publication?;
        Ok(outcome)
    }
}

pub(super) async fn dispatch_host_tool_call<T: AgentToolDispatcher + ?Sized + 'static>(
    tools: &Arc<T>,
    call: crate::types::ToolCall,
    timeout_policy: ToolDispatchTimeoutPolicy,
    dispatch_context: crate::ToolDispatchContext,
) -> Result<ToolDispatchOutcome, AgentError> {
    let args = to_raw_value(&call.args).map_err(|err| {
        AgentError::InternalError(format!(
            "failed to serialize external tool-call arguments: {err}"
        ))
    })?;
    let view = ToolCallView {
        id: &call.id,
        name: &call.name,
        args: args.as_ref(),
    };
    let resolution_started = crate::time_compat::Instant::now();
    let caller_deadline = timeout_policy.timeout().map_or_else(
        || ToolDeadlineContributor::unbounded(ToolDeadlineOwner::DirectCaller),
        |timeout| ToolDeadlineContributor::finite(ToolDeadlineOwner::DirectCaller, timeout),
    );
    let resolution_context = match ToolDeadlineChain::new(vec![caller_deadline])
        .map(ToolExecutionResolutionContext::new)
    {
        Ok(context) => context,
        Err(error) => {
            return Ok(crate::ops::terminal_tool_outcome_for_error(
                call.id,
                ToolError::from(ToolExecutionResolutionError::Deadline(error)),
            ));
        }
    };
    let plan = match crate::resolve_tool_execution_plan_fenced(
        tools,
        view,
        &dispatch_context,
        &resolution_context,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return Ok(crate::ops::terminal_tool_outcome_for_error(
                call.id,
                ToolError::from(error),
            ));
        }
    };
    if let Err(error) = tools.validate_resolved_execution_plan(view, &resolution_context, &plan) {
        return Ok(crate::ops::terminal_tool_outcome_for_error(
            call.id,
            ToolError::from(error),
        ));
    }
    let effective_timeout = plan.effective_timeout();
    tracing::debug!(
        tool = %call.name,
        execution_mode = ?plan.mode(),
        effective_deadline_ms = ?effective_timeout
            .map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
        deadline_winner = ?plan
            .deadlines()
            .winner()
            .map(|winner| winner.owner().as_str()),
        deadline_chain = %plan.deadlines().diagnostic(),
        "resolved external tool execution plan"
    );
    let remaining_timeout =
        effective_timeout.map(|timeout| timeout.saturating_sub(resolution_started.elapsed()));
    let advertised_timeout_ms =
        effective_timeout.map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX));
    let dispatch_result = match remaining_timeout {
        Some(timeout) if timeout.is_zero() => Err(crate::error::ToolError::timeout(
            call.name.clone(),
            advertised_timeout_ms.unwrap_or(u64::MAX),
        )),
        Some(timeout) => {
            match tokio::time::timeout(
                timeout,
                crate::dispatch_tool_execution_plan_fenced(tools, view, &dispatch_context, &plan),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(crate::error::ToolError::timeout(
                    call.name.clone(),
                    advertised_timeout_ms.unwrap_or(u64::MAX),
                )),
            }
        }
        None => {
            crate::dispatch_tool_execution_plan_fenced(tools, view, &dispatch_context, &plan).await
        }
    };

    match dispatch_result {
        Ok(mut outcome) => {
            outcome.clear_terminal_cause();
            if outcome.result.tool_use_id.is_empty() {
                outcome.result.tool_use_id = call.id;
            }
            Ok(outcome)
        }
        Err(error) if error.is_callback_pending() => {
            let (tool_name, args) = error.as_callback_pending().ok_or_else(|| {
                AgentError::InternalError(
                    "callback classification lost its exact payload".to_string(),
                )
            })?;
            Err(AgentError::callback_pending_with_settlement(
                crate::error::PendingCallbackToolCall {
                    tool_use_id: call.id,
                    tool_name: tool_name.to_owned(),
                    args: args.clone(),
                    settlement_failures: error.settlement_failures().cloned().collect(),
                },
            ))
        }
        Err(error)
            if matches!(
                error.primary_error(),
                ToolError::OperationObservationUnavailable
            ) =>
        {
            Err(AgentError::tool(error))
        }
        Err(error) => Ok(crate::ops::terminal_tool_outcome_for_error(call.id, error)),
    }
}
